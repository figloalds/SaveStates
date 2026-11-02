//! One background thread per project: git work runs there, in submission order, and each job's
//! result comes back to the UI thread through a [`Poster`].
//!
//! The UI thread never runs git (spec §7.2). Raven's own job pool is small and shared with its
//! renderer, so git work gets a thread of its own instead (one per open project).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;

use savestates_core::Project;

/// Hands a result to the UI thread (in the app: `ui_raven::task::post`).
pub type Poster = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

/// What a job gives back: the closure to run on the UI thread.
type Posted = Box<dyn FnOnce() + Send>;
type JobFn = Box<dyn FnOnce(&Project) -> Posted + Send>;

/// The diff library recursed past the default 2 MB stack in tests (Task 5).
const STACK_SIZE: usize = 16 * 1024 * 1024;

enum Job {
    Plain(JobFn),
    /// A refresh, with the generation it was submitted as.
    Refresh(u64, JobFn),
}

/// The background thread of one project. Dropping it lets the thread finish the job it is
/// running and stop; the jobs still queued are dropped.
pub struct ProjectWorker {
    jobs: Sender<Job>,
    /// The generation of the newest refresh submitted.
    generation: Arc<AtomicU64>,
}

impl ProjectWorker {
    /// Starts the thread that runs `project`'s jobs and posts their results with `post`.
    pub fn spawn(project: Project, post: Poster) -> ProjectWorker {
        let (jobs, queue) = mpsc::channel::<Job>();
        let generation = Arc::new(AtomicU64::new(0));
        let newest = generation.clone();
        let run = move || {
            while let Ok(job) = queue.recv() {
                match job {
                    Job::Plain(job) => post(job(&project)),
                    Job::Refresh(asked, job) => {
                        // A newer refresh is queued: this one would only be thrown away.
                        if asked != newest.load(Ordering::SeqCst) {
                            continue;
                        }
                        let result = job(&project);
                        // A newer refresh was asked for while this one ran: it will tell the
                        // UI something more recent, so this result is dropped.
                        if asked == newest.load(Ordering::SeqCst) {
                            post(result);
                        }
                    }
                }
            }
        };
        let spawned = thread::Builder::new()
            .name("savestates-project".to_owned())
            .stack_size(STACK_SIZE)
            .spawn(run);
        if let Err(error) = spawned {
            // Without a thread the jobs go nowhere: the project shows no history and no changes,
            // which is what a tab looks like while it loads. Nothing more useful can be done.
            eprintln!("could not start a project worker: {error}");
        }
        ProjectWorker { jobs, generation }
    }

    /// Runs `job` on the project's thread after the jobs submitted before it, and posts the
    /// closure it returns.
    pub fn submit(&self, job: impl FnOnce(&Project) -> Box<dyn FnOnce() + Send> + Send + 'static) {
        // A send fails only when the thread is gone, and then there is nobody to tell.
        let _ = self.jobs.send(Job::Plain(Box::new(job)));
    }

    /// Like [`submit`](Self::submit), for a job that reads the project's state: when another
    /// refresh is submitted before this one's result is posted, this result is dropped (and the
    /// job does not run at all if it has not started).
    pub fn submit_refresh(
        &self,
        job: impl FnOnce(&Project) -> Box<dyn FnOnce() + Send> + Send + 'static,
    ) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.jobs.send(Job::Refresh(generation, Box::new(job)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use savestates_core::start_saving_folder;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    type Collected = Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>;

    fn project(dir: &tempfile::TempDir) -> Project {
        let git = savestates_git::locate().expect("git is installed");
        start_saving_folder(&git, dir.path()).expect("a new project")
    }

    /// A poster that keeps the results instead of handing them to a UI thread.
    fn collector() -> (Poster, Collected) {
        let collected: Collected = Arc::default();
        let poster: Poster = {
            let collected = collected.clone();
            Arc::new(move |result| collected.lock().unwrap().push(result))
        };
        (poster, collected)
    }

    /// Waits until every job submitted so far has run.
    fn drain(worker: &ProjectWorker) {
        let (done, wait) = mpsc::channel();
        worker.submit(move |_| {
            done.send(()).unwrap();
            Box::new(|| {})
        });
        wait.recv_timeout(Duration::from_secs(10))
            .expect("the worker runs its jobs");
    }

    thread_local! {
        /// The tags of the results run on this test's thread, in the order they ran.
        static RAN: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Runs the collected results in the order they were posted (as the UI thread would) and
    /// returns the tags they recorded.
    fn run_all(collected: &Collected) -> Vec<u32> {
        let results: Vec<_> = collected.lock().unwrap().drain(..).collect();
        for result in results {
            result();
        }
        RAN.with_borrow_mut(std::mem::take)
    }

    /// A result that records `tag` when it runs.
    fn tagged(tag: u32) -> Box<dyn FnOnce() + Send> {
        Box::new(move || RAN.with_borrow_mut(|ran| ran.push(tag)))
    }

    #[test]
    fn jobs_run_in_submission_order_on_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let project = project(&dir);
        let root = project.root().to_path_buf();
        let (poster, collected) = collector();
        let worker = ProjectWorker::spawn(project, poster);

        let seen: Arc<Mutex<Vec<u32>>> = Arc::default();
        for tag in 0..20 {
            let (seen, root) = (seen.clone(), root.clone());
            worker.submit(move |p| {
                assert_eq!(p.root(), root);
                seen.lock().unwrap().push(tag);
                tagged(tag)
            });
        }
        drain(&worker);

        assert_eq!(*seen.lock().unwrap(), (0..20).collect::<Vec<_>>());
        let mut ran = run_all(&collected);
        // `drain`'s own result is the last one and records nothing.
        ran.retain(|tag| *tag < 20);
        assert_eq!(ran, (0..20).collect::<Vec<_>>());
    }

    #[test]
    fn a_refresh_waiting_behind_a_newer_one_is_not_posted() {
        let dir = tempfile::tempdir().unwrap();
        let (poster, collected) = collector();
        let worker = ProjectWorker::spawn(project(&dir), poster);

        // Hold the worker so that both refreshes are queued before either runs.
        let (release, hold) = mpsc::channel::<()>();
        worker.submit(move |_| {
            hold.recv_timeout(Duration::from_secs(10)).unwrap();
            tagged(1)
        });
        worker.submit_refresh(|_| tagged(2));
        worker.submit_refresh(|_| tagged(3));
        worker.submit(|_| tagged(4));
        release.send(()).unwrap();
        drain(&worker);

        assert_eq!(run_all(&collected), [1, 3, 4]);
    }

    #[test]
    fn a_refresh_that_finishes_after_a_newer_one_was_asked_for_is_not_posted() {
        let dir = tempfile::tempdir().unwrap();
        let (poster, collected) = collector();
        let worker = ProjectWorker::spawn(project(&dir), poster);

        // The first refresh is already running when the second one is submitted.
        let (started, running) = mpsc::channel::<()>();
        let (release, hold) = mpsc::channel::<()>();
        worker.submit_refresh(move |_| {
            started.send(()).unwrap();
            hold.recv_timeout(Duration::from_secs(10)).unwrap();
            tagged(1)
        });
        running.recv_timeout(Duration::from_secs(10)).unwrap();
        worker.submit_refresh(|_| tagged(2));
        release.send(()).unwrap();
        drain(&worker);

        assert_eq!(run_all(&collected), [2]);
    }

    #[test]
    fn a_refresh_alone_is_posted() {
        let dir = tempfile::tempdir().unwrap();
        let (poster, collected) = collector();
        let worker = ProjectWorker::spawn(project(&dir), poster);
        worker.submit_refresh(|_| tagged(7));
        drain(&worker);
        assert_eq!(run_all(&collected), [7]);
    }
}
