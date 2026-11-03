use std::fs;

use regex::Regex;
use savestates_core::{
    KEEP_IGNORED_COMMENT, LoadError, OpenOutcome, Project, SaveId, commit_all, continue_from_here,
    finish_load, list_recovery, load, open_project,
};
use savestates_git::{git_path, refs_with_prefix, status};
use testkit::TestRepo;

const LOAD_ASIDE: &str = "refs/savestates/load-aside/";

fn open(repo: &TestRepo) -> Project {
    match open_project(&repo.exe(), repo.path()).unwrap() {
        OpenOutcome::Opened(project) => project,
        other => panic!("expected Opened, got {other:?}"),
    }
}

fn save(repo: &TestRepo, file: &str, msg: &str) -> SaveId {
    repo.write(file, msg);
    SaveId(repo.commit_all(msg))
}

fn read(repo: &TestRepo, rel: &str) -> String {
    fs::read_to_string(repo.path().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn head(repo: &TestRepo) -> String {
    repo.git(&["rev-parse", "HEAD"]).trim().to_owned()
}

fn branch(repo: &TestRepo) -> String {
    repo.git(&["branch", "--show-current"]).trim().to_owned()
}

fn branches(repo: &TestRepo) -> Vec<String> {
    repo.git(&["branch", "--format=%(refname:short)"])
        .lines()
        .map(str::to_owned)
        .collect()
}

fn markers(repo: &TestRepo) -> Vec<String> {
    refs_with_prefix(&repo.repo(), LOAD_ASIDE)
        .unwrap()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

fn clean(repo: &TestRepo) -> bool {
    status(&repo.repo()).unwrap().entries.is_empty()
}

/// Asserts the folder's files (ignored ones left out) are exactly those of the save `rev`, with
/// the same content.
fn assert_folder_matches(repo: &TestRepo, rev: &str) {
    let lines =
        |args: &[&str]| -> Vec<String> { repo.git(args).lines().map(str::to_owned).collect() };
    let deleted = lines(&["ls-files", "--deleted"]);
    let mut on_disk: Vec<String> =
        lines(&["ls-files", "--cached", "--others", "--exclude-standard"])
            .into_iter()
            .filter(|name| !deleted.contains(name))
            .collect();
    on_disk.sort();
    let saved = lines(&["ls-tree", "-r", "--name-only", rev]);
    assert_eq!(on_disk, saved, "the same files as {rev}");
    for name in saved {
        assert_eq!(
            read(repo, &name),
            repo.git(&["show", &format!("{rev}:{name}")]),
            "{name}"
        );
    }
}

/// `main` with three saves: m1 writes `a.txt`, m2 `b.txt`, m3 `c.txt`.
fn three_saves(repo: &TestRepo) -> [SaveId; 3] {
    [
        save(repo, "a.txt", "m1"),
        save(repo, "b.txt", "m2"),
        save(repo, "c.txt", "m3"),
    ]
}

fn app_branch() -> Regex {
    Regex::new(r"^savestates/[a-z]+-[a-z]+(-[0-9]+)?$").unwrap()
}

#[test]
fn loading_mid_line_starts_a_new_timeline() {
    let repo = TestRepo::new();
    let [_, m2, m3] = three_saves(&repo);
    let out = load(&open(&repo), &m2, None).unwrap();
    assert_eq!(head(&repo), m2.0);
    assert!(app_branch().is_match(&branch(&repo)), "{}", branch(&repo));
    assert_eq!(out.branch, branch(&repo));
    assert!(out.started);
    assert_eq!(out.origin.save, m3);
    assert_eq!(out.origin.branch.as_deref(), Some("main"));
    assert_eq!(out.removed, None);
    assert!(out.left_over.is_empty());
    assert_folder_matches(&repo, &m2.0);
}

#[test]
fn loading_the_last_save_of_a_timeline_switches_to_it() {
    let repo = TestRepo::new();
    let [m1, ..] = three_saves(&repo);
    repo.git(&["switch", "-q", "-c", "exp", &m1.0]);
    save(&repo, "e.txt", "e1");
    let e2 = save(&repo, "e.txt", "e2");
    repo.git(&["switch", "-q", "main"]);
    let out = load(&open(&repo), &e2, None).unwrap();
    assert_eq!(branch(&repo), "exp");
    assert_eq!(out.branch, "exp");
    assert!(!out.started);
    assert_folder_matches(&repo, &e2.0);
}

#[test]
fn unsaved_changes_block_load_and_touch_nothing() {
    let repo = TestRepo::new();
    let [m1, _, m3] = three_saves(&repo);
    let p = open(&repo);

    repo.write("a.txt", "edited");
    assert!(matches!(
        load(&p, &m1, None),
        Err(LoadError::HasUnsavedChanges)
    ));
    assert_eq!(head(&repo), m3.0);
    assert_eq!(branch(&repo), "main");
    assert_eq!(read(&repo, "a.txt"), "edited");

    repo.git(&["checkout", "-q", "--", "a.txt"]);
    repo.write("new.txt", "mine");
    assert!(matches!(
        load(&p, &m1, None),
        Err(LoadError::HasUnsavedChanges)
    ));
    assert_eq!(head(&repo), m3.0);
    assert_eq!(read(&repo, "new.txt"), "mine");
    assert_eq!(branches(&repo), ["main"]);
}

#[test]
fn leaving_an_empty_app_timeline_deletes_it() {
    let repo = TestRepo::new();
    let [_, m2, m3] = three_saves(&repo);
    let p = open(&repo);
    let x = load(&p, &m2, None).unwrap().branch;
    let out = load(&p, &m3, None).unwrap();
    assert_eq!(branch(&repo), "main");
    assert!(!out.started);
    assert_eq!(out.removed.as_deref(), Some(x.as_str()));
    assert!(!branches(&repo).contains(&x));
    assert_folder_matches(&repo, &m3.0);
}

#[test]
fn a_timeline_with_a_save_is_kept_when_left() {
    let repo = TestRepo::new();
    let [_, m2, m3] = three_saves(&repo);
    let p = open(&repo);
    let x = load(&p, &m2, None).unwrap().branch;
    repo.write("x.txt", "mine");
    commit_all(&p, "Minha ideia", &["x.txt".to_owned()], 50).unwrap();
    let out = load(&p, &m3, None).unwrap();
    assert_eq!(out.removed, None);
    assert!(branches(&repo).contains(&x));
    assert_eq!(
        repo.git(&["config", &format!("branch.{x}.description")])
            .trim(),
        "Minha ideia"
    );
}

#[test]
fn desfazer_returns_to_where_the_friend_was() {
    let repo = TestRepo::new();
    let [_, m2, m3] = three_saves(&repo);
    let p = open(&repo);
    let o = load(&p, &m2, None).unwrap().origin;
    let back = load(&p, &o.save, o.branch.as_deref()).unwrap();
    assert_eq!(branch(&repo), "main");
    assert_eq!(head(&repo), m3.0);
    assert!(!back.started);
    assert_folder_matches(&repo, &m3.0);
}

/// m1 tracks `.env` (`old`); m2 stops tracking it and ignores it; `.env` on disk is `real`.
fn env_setup(repo: &TestRepo) -> [SaveId; 2] {
    repo.write("a.txt", "a");
    repo.write(".env", "old");
    let m1 = SaveId(repo.commit_all("m1"));
    repo.git(&["rm", "-q", "--cached", ".env"]);
    repo.write(".gitignore", ".env\n");
    let m2 = SaveId(repo.commit_all("m2"));
    repo.write(".env", "real");
    assert!(clean(repo));
    [m1, m2]
}

#[test]
fn an_ignored_file_in_the_way_is_set_aside_and_comes_back() {
    let repo = TestRepo::new();
    let [m1, m2] = env_setup(&repo);
    let p = open(&repo);

    let out = load(&p, &m1, None).unwrap();
    assert_eq!(read(&repo, ".env"), "old");
    let aside = out.set_aside.expect("the real .env was set aside");
    assert_eq!(aside.files, [".env"]);
    assert_eq!(markers(&repo).len(), 1);
    assert_eq!(
        markers(&repo)[0].strip_prefix(LOAD_ASIDE),
        aside.ref_name.strip_prefix("refs/savestates/recovery/")
    );

    let back = load(&p, &m2, None).unwrap();
    assert_eq!(branch(&repo), "main");
    assert_eq!(read(&repo, ".env"), "real");
    assert_eq!(back.brought_back, 1);
    assert!(list_recovery(&p).unwrap().is_empty());
    assert!(markers(&repo).is_empty());
    assert!(clean(&repo));
}

#[test]
fn a_set_aside_file_waits_in_recovery_while_its_path_is_taken() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a");
    repo.write(".env", "old");
    let m1 = SaveId(repo.commit_all("m1"));
    // `old2` still tracks `.env`, in a save of its own.
    repo.git(&["switch", "-q", "-c", "old2"]);
    let old2 = save(&repo, "o.txt", "o1");
    repo.git(&["switch", "-q", "main"]);
    repo.git(&["rm", "-q", "--cached", ".env"]);
    repo.write(".gitignore", ".env\n");
    repo.commit_all("m2");
    repo.write(".env", "real");
    assert!(clean(&repo));
    let p = open(&repo);

    load(&p, &m1, None).unwrap();
    let out = load(&p, &old2, None).unwrap();
    assert_eq!(branch(&repo), "old2");
    assert_eq!(out.brought_back, 0);
    assert_eq!(read(&repo, ".env"), "old");
    let entry = list_recovery(&p)
        .unwrap()
        .into_iter()
        .find(|e| e.files == [".env"])
        .expect("the real .env waits in recovery");
    assert_eq!(
        repo.git(&["show", &format!("{}^3:.env", entry.ref_name)]),
        "real"
    );
    assert_eq!(markers(&repo).len(), 1);
}

#[test]
fn what_was_ignored_stays_ignored() {
    let repo = TestRepo::new();
    let m1 = save(&repo, "a.txt", "m1");
    repo.write(".gitignore", "node_modules/\n");
    repo.commit_all("m2");
    repo.write("node_modules/p/i.js", "i");
    repo.write("node_modules/q.js", "q");
    assert!(clean(&repo));
    let p = open(&repo);

    let out = load(&p, &m1, None).unwrap();
    assert!(clean(&repo), "{:?}", status(&repo.repo()).unwrap().entries);
    assert_eq!(out.kept_ignored, ["/node_modules/"]);
    assert!(out.left_over.is_empty());
    let exclude = fs::read_to_string(git_path(&repo.repo(), "info/exclude").unwrap()).unwrap();
    let lines: Vec<&str> = exclude.lines().collect();
    let at = lines
        .iter()
        .position(|l| *l == KEEP_IGNORED_COMMENT)
        .expect("the comment line");
    assert_eq!(lines.get(at + 1), Some(&"/node_modules/"));
    assert_eq!(read(&repo, "node_modules/q.js"), "q");

    load(&p, &out.origin.save, out.origin.branch.as_deref()).unwrap();
    assert_eq!(branch(&repo), "main");
    assert_eq!(read(&repo, "node_modules/p/i.js"), "i");
}

/// Git 2.50 on Windows, when a file it must rewrite is held open with no sharing: it prints
/// `error: unable to unlink old 'a.txt'`, still exits 0, and moves HEAD and the index to the
/// target; the file keeps its old content. So Load is `Ok` and reports the file left over.
#[cfg(windows)]
#[test]
fn a_locked_file_is_reported_and_finished_later() {
    use std::os::windows::fs::OpenOptionsExt;

    let repo = TestRepo::new();
    let m1 = save(&repo, "a.txt", "m1");
    save(&repo, "b.txt", "m2");
    let m3 = save(&repo, "a.txt", "m3");
    let p = open(&repo);
    // Just saved, `a.txt` is "racily clean": status would have to read it, which the lock
    // forbids. Like a file saved a while ago, it is made older than the index first.
    fs::OpenOptions::new()
        .write(true)
        .open(repo.path().join("a.txt"))
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
        .unwrap();
    repo.git(&["update-index", "-q", "--refresh"]);
    assert!(clean(&repo));

    let handle = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(repo.path().join("a.txt"))
        .unwrap();
    let result = load(&p, &m1, None);
    match result {
        Ok(out) => {
            assert_eq!(head(&repo), m1.0);
            assert_eq!(out.left_over, ["a.txt"]);
            drop(handle);
            assert_eq!(finish_load(&p, &m3).unwrap(), Vec::<String>::new());
            assert!(clean(&repo));
            assert_folder_matches(&repo, &m1.0);
        }
        Err(LoadError::SwitchFailed { .. }) => {
            drop(handle);
            assert_eq!(head(&repo), m3.0);
            assert!(clean(&repo));
            panic!(
                "git 2.50 was seen to finish the switch and leave the file: a refusal means git changed"
            );
        }
        Err(e) => panic!("unexpected {e:?}"),
    }
}

#[test]
fn finishing_later_leaves_files_the_friend_changed_since() {
    let repo = TestRepo::new();
    let m1 = save(&repo, "a.txt", "m1");
    save(&repo, "b.txt", "m2");
    let m3 = save(&repo, "a.txt", "m3");
    // As if git had left `a.txt` and `b.txt` at m3 after moving to m1, and the friend then
    // edited `a.txt`.
    repo.git(&["switch", "-q", "--detach", &m1.0]);
    repo.write("a.txt", "edited after");
    repo.write("b.txt", "m2");
    let p = open(&repo);
    assert_eq!(finish_load(&p, &m3).unwrap(), ["a.txt"]);
    assert_eq!(read(&repo, "a.txt"), "edited after");
    assert!(!repo.path().join("b.txt").exists());
}

#[test]
fn a_failing_post_checkout_hook_is_only_a_warning() {
    let repo = TestRepo::new();
    let [_, m2, _] = three_saves(&repo);
    let hook = git_path(&repo.repo(), "hooks/post-checkout").unwrap();
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, "#!/bin/sh\necho hook said no >&2\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = load(&open(&repo), &m2, None).unwrap();
    assert!(out.warning.is_some());
    assert_eq!(head(&repo), m2.0);
    assert!(out.started);
}

#[test]
fn a_branch_named_savestates_blocks_only_new_timelines() {
    let repo = TestRepo::new();
    let [m1, m2, m3] = three_saves(&repo);
    repo.git(&["branch", "savestates", &m1.0]);
    repo.git(&["switch", "-q", "-c", "exp", &m1.0]);
    let e1 = save(&repo, "e.txt", "e1");
    repo.git(&["switch", "-q", "main"]);
    let p = open(&repo);

    assert!(matches!(load(&p, &m2, None), Err(LoadError::PrefixTaken)));
    assert_eq!(head(&repo), m3.0);
    assert_eq!(branch(&repo), "main");
    assert!(clean(&repo));
    assert_eq!(branches(&repo), ["exp", "main", "savestates"]);

    load(&p, &e1, None).unwrap();
    assert_eq!(branch(&repo), "exp");
}

#[test]
fn a_worktree_branch_tip_starts_a_new_timeline() {
    let repo = TestRepo::new();
    let [m1, ..] = three_saves(&repo);
    repo.git(&["switch", "-q", "-c", "side", &m1.0]);
    let s1 = save(&repo, "s.txt", "s1");
    repo.git(&["switch", "-q", "main"]);
    let wt = repo.path().parent().unwrap().join(format!(
        "wt-{}",
        repo.path().file_name().unwrap().to_string_lossy()
    ));
    repo.git(&["worktree", "add", "-q", wt.to_str().unwrap(), "side"]);

    let out = load(&open(&repo), &s1, None).unwrap();
    assert!(out.started);
    assert!(app_branch().is_match(&out.branch), "{}", out.branch);
    assert_eq!(branch(&repo), out.branch);
    assert_eq!(head(&repo), s1.0);
    let _ = fs::remove_dir_all(wt);
}

#[test]
fn continuing_from_a_bare_save() {
    let repo = TestRepo::new();
    let [_, m2, m3] = three_saves(&repo);
    let p = open(&repo);

    repo.git(&["switch", "-q", "--detach", &m2.0]);
    repo.write("a.txt", "still here");
    let b = continue_from_here(&p).unwrap();
    assert!(app_branch().is_match(&b), "{b}");
    assert_eq!(branch(&repo), b);
    assert_eq!(head(&repo), m2.0);
    assert_eq!(read(&repo, "a.txt"), "still here");

    repo.git(&["checkout", "-q", "--", "a.txt"]);
    repo.git(&["switch", "-q", "--detach", &m3.0]);
    assert_eq!(continue_from_here(&p).unwrap(), "main");
    assert_eq!(branch(&repo), "main");
}

#[test]
fn continuing_is_refused_unless_on_a_bare_save() {
    let repo = TestRepo::new();
    three_saves(&repo);
    assert!(matches!(
        continue_from_here(&open(&repo)),
        Err(LoadError::NotNormal)
    ));
}

/// With `HEAD.lock` held (another git program at work), git writes the target's files and index
/// and only then fails to move HEAD. Load puts everything back, the set-aside `.env` included.
#[test]
fn a_switch_that_does_not_move_puts_everything_back() {
    let repo = TestRepo::new();
    let [m1, m2] = env_setup(&repo);
    let p = open(&repo);
    let lock = repo.path().join(".git").join("HEAD.lock");
    fs::write(&lock, "").unwrap();

    let result = load(&p, &m1, None);
    fs::remove_file(&lock).unwrap();
    assert!(
        matches!(result, Err(LoadError::SwitchFailed { .. })),
        "{result:?}"
    );
    assert_eq!(head(&repo), m2.0);
    assert_eq!(branch(&repo), "main");
    assert!(clean(&repo), "{:?}", status(&repo.repo()).unwrap().entries);
    assert_folder_matches(&repo, &m2.0);
    assert_eq!(read(&repo, ".env"), "real");
    assert!(list_recovery(&p).unwrap().is_empty());
    assert!(markers(&repo).is_empty());
    assert_eq!(branches(&repo), ["main"]);
}
