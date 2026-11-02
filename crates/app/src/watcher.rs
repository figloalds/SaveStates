//! Watches a project folder so that "Now" follows edits made by other programs (spec §6.1).

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify_debouncer_mini::notify::{self, RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};

/// Changes closer together than this are reported once.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// Whether a change at `rel` (a path inside the project folder, with `/` or `\`) can change what
/// the app shows: any work file, and among git's own files only the ones that say which save the
/// project is at and which timelines exist.
pub fn is_relevant(rel: &str) -> bool {
    let rel = rel.replace('\\', "/");
    match rel.strip_prefix(".git/") {
        None => rel != ".git",
        Some(inside) => inside == "HEAD" || inside.starts_with("refs/"),
    }
}

/// Keeps a folder watched until dropped.
pub struct WatchHandle {
    _debouncer: Debouncer<RecommendedWatcher>,
}

/// Watches everything under `root` and calls `on_change` (on the watcher's thread) at most
/// once per burst of relevant changes.
pub fn watch(root: &Path, on_change: impl Fn() + Send + 'static) -> notify::Result<WatchHandle> {
    let base: PathBuf = root.to_path_buf();
    let mut debouncer = new_debouncer(DEBOUNCE, move |result: DebounceEventResult| {
        // An error from the system watcher (a dropped event buffer, for one) may hide a change,
        // so it is treated as one.
        let changed = match result {
            Ok(events) => events.iter().any(|event| {
                event
                    .path
                    .strip_prefix(&base)
                    .is_ok_and(|rel| is_relevant(&rel.to_string_lossy()))
            }),
            Err(_) => true,
        };
        if changed {
            on_change();
        }
    })?;
    debouncer.watcher().watch(root, RecursiveMode::Recursive)?;
    Ok(WatchHandle {
        _debouncer: debouncer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_files_and_the_current_save_are_relevant() {
        assert!(is_relevant("src/a.js"));
        assert!(is_relevant(".git/HEAD"));
        assert!(is_relevant(".git/refs/heads/main"));
    }

    #[test]
    fn other_internal_files_are_not() {
        assert!(!is_relevant(".git/index"));
        assert!(!is_relevant(".git/index.lock"));
        assert!(!is_relevant(".git/objects/ab/cd"));
        assert!(!is_relevant(".git/logs/HEAD"));
    }

    #[test]
    fn windows_separators_count_too() {
        assert!(is_relevant(r"src\a.js"));
        assert!(is_relevant(r".git\refs\heads\main"));
        assert!(!is_relevant(r".git\objects\ab\cd"));
        assert!(!is_relevant(".git"));
    }

    #[test]
    fn a_work_file_change_is_reported_and_an_internal_one_is_not() {
        use std::sync::mpsc;

        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join(".git/objects/ab")).unwrap();
        let (tx, changes) = mpsc::channel();
        let _handle = watch(&root, move || {
            let _ = tx.send(());
        })
        .expect("the folder can be watched");

        std::fs::write(root.join(".git/objects/ab/cd"), "x").unwrap();
        std::fs::write(root.join(".git/index"), "x").unwrap();
        assert!(
            changes.recv_timeout(Duration::from_millis(1500)).is_err(),
            "internal files alone must not report a change"
        );

        std::fs::write(root.join("a.js"), "x").unwrap();
        assert!(
            changes.recv_timeout(Duration::from_secs(10)).is_ok(),
            "a work file change is reported"
        );
    }
}
