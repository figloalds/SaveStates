use std::fs;

use savestates_core::{
    BringBackError, Change, ChangeKind, CoreError, OpenOutcome, Project, SaveId, Side,
    UnsavedPolicy, bring_back_everything, bring_back_file, list_recovery, open_project,
    restore_recovery, snapshot,
};
use savestates_git::rm_cached;
use testkit::TestRepo;

fn open(repo: &TestRepo) -> Project {
    match open_project(&repo.exe(), repo.path()).unwrap() {
        OpenOutcome::Opened(project) => project,
        other => panic!("expected Opened, got {other:?}"),
    }
}

fn change(path: &str, kind: ChangeKind) -> Change {
    Change {
        path: path.to_owned(),
        kind,
    }
}

fn read(repo: &TestRepo, rel: &str) -> String {
    fs::read_to_string(repo.path().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn exists(repo: &TestRepo, rel: &str) -> bool {
    repo.path().join(rel).exists()
}

fn head(repo: &TestRepo) -> String {
    repo.git(&["rev-parse", "HEAD"]).trim().to_owned()
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

/// `a.txt` is `v1` in the first save and `v2` in the second.
fn two_saves() -> (TestRepo, SaveId, SaveId) {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    let s1 = SaveId(repo.commit_all("s1"));
    repo.write("a.txt", "v2");
    let s2 = SaveId(repo.commit_all("s2"));
    (repo, s1, s2)
}

/// `a.txt` and `keep.txt` in the first save; the second changes `a.txt`, adds `new.txt` and
/// removes `gone.txt`. `*.log` files are ignored. Left on the second save with a clean folder.
fn two_saves_with_adds_and_removes() -> (TestRepo, SaveId, SaveId) {
    let repo = TestRepo::new();
    repo.write(".gitignore", "*.log\n");
    repo.write("a.txt", "v1");
    repo.write("keep.txt", "keep");
    repo.write("gone.txt", "gone");
    let s1 = SaveId(repo.commit_all("s1"));
    repo.write("a.txt", "v2");
    repo.write("new.txt", "new");
    repo.remove("gone.txt");
    let s2 = SaveId(repo.commit_all("s2"));
    (repo, s1, s2)
}

#[test]
fn file_from_save() {
    let (repo, s1, s2) = two_saves();
    let p = open(&repo);

    let set_aside = bring_back_file(&p, &Side::Save(s1), "a.txt").unwrap();

    assert!(set_aside.is_none());
    assert_eq!(read(&repo, "a.txt"), "v1");
    assert_eq!(head(&repo), s2.0, "HEAD did not move");
    assert_eq!(
        snapshot(&p).changes,
        vec![change("a.txt", ChangeKind::Changed)]
    );
}

#[test]
fn file_from_before_a_save() {
    let (repo, _s1, s2) = two_saves();
    let p = open(&repo);

    bring_back_file(&p, &Side::Before(s2.clone()), "a.txt").unwrap();

    assert_eq!(read(&repo, "a.txt"), "v1");
    assert_eq!(head(&repo), s2.0, "HEAD did not move");
}

#[test]
fn file_with_unsaved_edit_is_recoverable() {
    let (repo, s1, _s2) = two_saves();
    repo.write("a.txt", "mine");
    repo.write("other.txt", "unrelated");
    let p = open(&repo);

    let entry = bring_back_file(&p, &Side::Save(s1), "a.txt")
        .unwrap()
        .expect("the unsaved edit was set aside");

    assert!(entry.files.contains(&"a.txt".to_owned()), "{entry:?}");
    assert_eq!(entry.files.len(), 1, "only the one file: {entry:?}");
    assert_eq!(read(&repo, "a.txt"), "v1");
    assert_eq!(read(&repo, "other.txt"), "unrelated");
    let kept = repo.git(&["show", &format!("{}:a.txt", entry.ref_name)]);
    assert_eq!(kept, "mine");

    // And it can be had back.
    restore_recovery(&p, &entry.ref_name).unwrap();
    assert_eq!(read(&repo, "a.txt"), "mine");
}

#[test]
fn before_of_created_file_deletes() {
    let (repo, _s1, _s2) = two_saves();
    repo.write("b.txt", "created");
    let s3 = SaveId(repo.commit_all("s3"));
    let p = open(&repo);

    let set_aside = bring_back_file(&p, &Side::Before(s3), "b.txt").unwrap();

    assert!(set_aside.is_none());
    assert!(!exists(&repo, "b.txt"));
    assert_eq!(
        snapshot(&p).changes,
        vec![change("b.txt", ChangeKind::Deleted)]
    );
}

#[test]
fn before_a_root_save_has_no_files() {
    let (repo, s1, _s2) = two_saves();
    let p = open(&repo);

    bring_back_file(&p, &Side::Before(s1), "a.txt").unwrap();

    assert!(!exists(&repo, "a.txt"));
}

#[test]
fn save_without_file_deletes() {
    let (repo, s1, _s2) = two_saves();
    repo.write("b.txt", "created");
    repo.commit_all("s3");
    let p = open(&repo);

    bring_back_file(&p, &Side::Save(s1), "b.txt").unwrap();

    assert!(!exists(&repo, "b.txt"));
}

#[test]
fn absent_everywhere_is_a_no_op() {
    let (repo, s1, _s2) = two_saves();
    let p = open(&repo);

    let set_aside = bring_back_file(&p, &Side::Save(s1), "never-existed.txt").unwrap();

    assert!(set_aside.is_none());
    assert!(snapshot(&p).changes.is_empty());
}

#[test]
fn deleted_file_comes_back() {
    let (repo, s1, _s2) = two_saves_with_adds_and_removes();
    let p = open(&repo);

    bring_back_file(&p, &Side::Save(s1), "gone.txt").unwrap();

    assert_eq!(read(&repo, "gone.txt"), "gone");
    assert_eq!(
        snapshot(&p).changes,
        vec![change("gone.txt", ChangeKind::New)]
    );
}

#[test]
fn accented_path() {
    let repo = TestRepo::new();
    repo.write("Área/Ação.md", "v1");
    repo.write("Área/Outra.md", "v1");
    let s1 = SaveId(repo.commit_all("s1"));
    repo.write("Área/Ação.md", "v2");
    repo.write("Área/Outra.md", "v2");
    repo.commit_all("s2");
    let p = open(&repo);

    bring_back_file(&p, &Side::Save(s1.clone()), "Área/Ação.md").unwrap();
    assert_eq!(read(&repo, "Área/Ação.md"), "v1");
    assert_eq!(read(&repo, "Área/Outra.md"), "v2");
    assert_eq!(
        snapshot(&p).changes,
        vec![change("Área/Ação.md", ChangeKind::Changed)]
    );

    // With an unsaved edit to the same file, the recovery entry names it exactly.
    repo.write("Área/Ação.md", "mine");
    let entry = bring_back_file(&p, &Side::Save(s1), "Área/Ação.md")
        .unwrap()
        .unwrap();
    assert_eq!(entry.files, vec!["Área/Ação.md".to_owned()]);
    assert_eq!(read(&repo, "Área/Ação.md"), "v1");
}

#[test]
fn file_name_with_glob_characters_is_literal() {
    let repo = TestRepo::new();
    repo.write("src/[id].js", "v1");
    repo.write("src/i.js", "v1");
    let s1 = SaveId(repo.commit_all("s1"));
    repo.write("src/[id].js", "v2");
    repo.write("src/i.js", "v2");
    repo.commit_all("s2");
    let p = open(&repo);

    bring_back_file(&p, &Side::Save(s1), "src/[id].js").unwrap();

    assert_eq!(read(&repo, "src/[id].js"), "v1");
    assert_eq!(read(&repo, "src/i.js"), "v2", "the glob did not reach it");
}

#[test]
fn now_is_not_a_place_to_bring_back_from() {
    let (repo, _s1, _s2) = two_saves();
    let p = open(&repo);

    match bring_back_file(&p, &Side::Now, "a.txt") {
        Err(CoreError::NotFound(what)) => assert_eq!(what, "now"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn paths_outside_the_project_are_refused() {
    let (repo, s1, _s2) = two_saves();
    let outside = repo
        .path()
        .parent()
        .unwrap()
        .join("outside-the-project.txt");
    fs::write(&outside, "do not delete").unwrap();
    let p = open(&repo);

    for path in ["../outside-the-project.txt", "", "/abs.txt", "a/../../x"] {
        assert!(
            matches!(
                bring_back_file(&p, &Side::Save(s1.clone()), path),
                Err(CoreError::NotFound(_))
            ),
            "{path:?}"
        );
    }
    assert!(outside.exists());
    fs::remove_file(outside).unwrap();
}

#[test]
fn everything_requires_clean() {
    let (repo, s1, _s2) = two_saves();
    repo.write("a.txt", "mine");
    let p = open(&repo);

    let result = bring_back_everything(&p, &s1, UnsavedPolicy::RequireClean);

    assert!(
        matches!(result, Err(BringBackError::HasUnsavedChanges)),
        "{result:?}"
    );
    assert_eq!(read(&repo, "a.txt"), "mine", "nothing was touched");
    assert!(list_recovery(&p).unwrap().is_empty());
}

#[test]
fn everything_matches_old_tree() {
    let (repo, s1, s2) = two_saves_with_adds_and_removes();
    repo.write("local.log", "keep me");
    let p = open(&repo);

    let set_aside = bring_back_everything(&p, &s1, UnsavedPolicy::RequireClean).unwrap();

    assert!(set_aside.is_none());
    assert_eq!(read(&repo, "a.txt"), "v1");
    assert_eq!(read(&repo, "keep.txt"), "keep");
    assert_eq!(read(&repo, "gone.txt"), "gone");
    assert!(!exists(&repo, "new.txt"));
    assert_eq!(read(&repo, "local.log"), "keep me");
    assert_eq!(head(&repo), s2.0, "HEAD did not move");
    assert_folder_matches(&repo, &s1.0);
    // Git lists the tracked changes before the untracked ones.
    let mut changes = snapshot(&p).changes;
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(
        changes,
        vec![
            change("a.txt", ChangeKind::Changed),
            change("gone.txt", ChangeKind::New),
            change("new.txt", ChangeKind::Deleted),
        ]
    );
}

#[test]
fn everything_moves_unsaved() {
    let (repo, s1, _s2) = two_saves_with_adds_and_removes();
    repo.write("a.txt", "mine");
    repo.write("scratch.txt", "scratch");
    let p = open(&repo);

    let entry = bring_back_everything(&p, &s1, UnsavedPolicy::MoveToRecovery)
        .unwrap()
        .expect("the unsaved changes were set aside");

    assert_eq!(
        entry.files,
        vec!["a.txt".to_owned(), "scratch.txt".to_owned()]
    );
    assert_eq!(read(&repo, "a.txt"), "v1");
    assert!(!exists(&repo, "scratch.txt"));
    assert!(!exists(&repo, "new.txt"));
    assert_folder_matches(&repo, &s1.0);
    assert_eq!(list_recovery(&p).unwrap(), vec![entry]);
}

#[test]
fn everything_on_a_clean_folder_with_move_policy_sets_nothing_aside() {
    let (repo, s1, _s2) = two_saves();
    let p = open(&repo);

    let set_aside = bring_back_everything(&p, &s1, UnsavedPolicy::MoveToRecovery).unwrap();

    assert!(set_aside.is_none());
    assert_eq!(read(&repo, "a.txt"), "v1");
}

/// `.env` is saved as `old secret` in the first save. The second stops saving it (out of the
/// index, ignored). The folder holds `CURRENT secret`, which git does not list as a change.
fn stopped_saving_env() -> (TestRepo, SaveId, SaveId) {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    repo.write(".env", "old secret");
    let s1 = SaveId(repo.commit_all("s1"));
    repo.write(".gitignore", ".env\n");
    let p = open(&repo);
    rm_cached(p.repo(), ".env", false).unwrap();
    let s2 = SaveId(repo.commit_all("s2"));
    repo.write(".env", "CURRENT secret");
    assert!(snapshot(&p).changes.is_empty(), "ignored, so not a change");
    (repo, s1, s2)
}

/// What the recovery entry `ref_name` holds for the ignored or new file `path`.
fn kept_untracked(repo: &TestRepo, ref_name: &str, path: &str) -> String {
    repo.git(&["show", &format!("{ref_name}^3:{path}")])
}

#[test]
fn everything_sets_aside_an_ignored_file_it_would_overwrite() {
    let (repo, s1, _s2) = stopped_saving_env();
    repo.write("local.log", "keep me");
    repo.write("node_modules/pkg/index.js", "module");
    repo.write(".gitignore", ".env\n*.log\nnode_modules/\n");
    repo.commit_all("s3");
    let p = open(&repo);

    let entry = bring_back_everything(&p, &s1, UnsavedPolicy::RequireClean)
        .unwrap()
        .expect("the ignored .env was set aside");

    assert_eq!(read(&repo, ".env"), "old secret");
    assert_eq!(
        entry.files,
        vec![".env".to_owned()],
        "only what is overwritten"
    );
    assert_eq!(
        kept_untracked(&repo, &entry.ref_name, ".env"),
        "CURRENT secret"
    );
    assert_eq!(read(&repo, "node_modules/pkg/index.js"), "module");
    assert_eq!(list_recovery(&p).unwrap().len(), 1);

    // Undoing brings the current content back.
    restore_recovery(&p, &entry.ref_name).unwrap();
    assert_eq!(read(&repo, ".env"), "CURRENT secret");
}

#[test]
fn everything_returns_the_unsaved_changes_entry_when_it_makes_two() {
    let (repo, s1, _s2) = stopped_saving_env();
    repo.write("a.txt", "mine");
    let p = open(&repo);

    let entry = bring_back_everything(&p, &s1, UnsavedPolicy::MoveToRecovery)
        .unwrap()
        .expect("something was set aside");

    assert_eq!(entry.files, vec!["a.txt".to_owned()], "the most recent one");
    let listed = list_recovery(&p).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0], entry, "newest first");
    assert_eq!(listed[1].files, vec![".env".to_owned()]);
    assert_eq!(
        kept_untracked(&repo, &listed[1].ref_name, ".env"),
        "CURRENT secret"
    );
}

#[test]
fn file_from_before_a_save_sets_aside_the_ignored_file_it_overwrites() {
    let (repo, _s1, s2) = stopped_saving_env();
    let p = open(&repo);

    let entry = bring_back_file(&p, &Side::Before(s2), ".env")
        .unwrap()
        .expect("the ignored .env was set aside");

    assert_eq!(read(&repo, ".env"), "old secret");
    assert_eq!(entry.files, vec![".env".to_owned()]);
    assert_eq!(
        kept_untracked(&repo, &entry.ref_name, ".env"),
        "CURRENT secret"
    );
}

#[test]
fn file_from_a_save_without_it_sets_aside_the_ignored_file_it_deletes() {
    let (repo, _s1, s2) = stopped_saving_env();
    let p = open(&repo);

    let entry = bring_back_file(&p, &Side::Save(s2), ".env")
        .unwrap()
        .expect("the ignored .env was set aside");

    assert!(!exists(&repo, ".env"));
    assert_eq!(entry.files, vec![".env".to_owned()]);
    assert_eq!(
        kept_untracked(&repo, &entry.ref_name, ".env"),
        "CURRENT secret"
    );
}

#[test]
fn file_leaves_other_ignored_files_alone() {
    let (repo, _s1, s2) = stopped_saving_env();
    repo.write(".gitignore", ".env\n*.log\n");
    repo.write("local.log", "keep me");
    let s3 = SaveId(repo.commit_all("s3"));
    let p = open(&repo);

    let set_aside = bring_back_file(&p, &Side::Save(s2), "a.txt").unwrap();

    assert!(set_aside.is_none());
    assert_eq!(read(&repo, ".env"), "CURRENT secret");
    assert_eq!(read(&repo, "local.log"), "keep me");
    assert!(list_recovery(&p).unwrap().is_empty());
    assert_eq!(head(&repo), s3.0);
}

/// `readme.md` in the first save is renamed to `README.md` in the second, on a file system and
/// with `core.ignorecase` that treat both spellings as one file. `README.md` has unsaved text.
#[cfg(windows)]
fn renamed_by_case_with_unsaved_text() -> (TestRepo, SaveId) {
    let repo = TestRepo::new();
    repo.git(&["config", "core.ignorecase", "true"]);
    repo.write("readme.md", "v1");
    let s1 = SaveId(repo.commit_all("s1"));
    repo.git(&["mv", "readme.md", "README.md"]);
    repo.commit_all("s2");
    repo.write("README.md", "MINE unsaved");
    assert_eq!(
        snapshot(&open(&repo)).changes,
        vec![change("README.md", ChangeKind::Changed)]
    );
    (repo, s1)
}

#[cfg(windows)]
#[test]
fn path_spelled_in_another_case_still_finds_the_unsaved_changes() {
    let (repo, s1) = renamed_by_case_with_unsaved_text();
    let p = open(&repo);

    let entry = bring_back_file(&p, &Side::Save(s1), "readme.md")
        .unwrap()
        .expect("the unsaved text was set aside");

    assert_eq!(entry.files, vec!["README.md".to_owned()]);
    assert_eq!(
        repo.git(&["show", &format!("{}:README.md", entry.ref_name)]),
        "MINE unsaved"
    );
    assert_eq!(read(&repo, "readme.md"), "v1");
}

#[cfg(windows)]
#[test]
fn path_spelled_in_another_case_is_set_aside_before_it_is_deleted() {
    let (repo, s1) = renamed_by_case_with_unsaved_text();
    let p = open(&repo);

    // Nothing came before the first save, so the file goes away.
    let entry = bring_back_file(&p, &Side::Before(s1), "readme.md")
        .unwrap()
        .expect("the unsaved text was set aside");

    assert!(!exists(&repo, "README.md"));
    assert_eq!(
        repo.git(&["show", &format!("{}:README.md", entry.ref_name)]),
        "MINE unsaved"
    );
}

/// The first save has a file `build` and `logs/a.txt`. The second removes both and ignores both
/// names. In the folder now: an ignored folder `build/` and an ignored file `logs`, so putting
/// the first save back needs the room that each of them takes.
fn ignored_paths_in_the_way_of_a_save() -> (TestRepo, SaveId, SaveId) {
    let repo = TestRepo::new();
    repo.write("keep.txt", "keep");
    repo.write("build", "a file");
    repo.write("logs/a.txt", "a");
    let s1 = SaveId(repo.commit_all("s1"));
    repo.git(&["rm", "-q", "build", "logs/a.txt"]);
    repo.write(".gitignore", "build\nlogs\n");
    let s2 = SaveId(repo.commit_all("s2"));
    repo.write("build/out.js", "compiled");
    repo.write("logs", "log text");
    assert!(
        snapshot(&open(&repo)).changes.is_empty(),
        "ignored, so not changes"
    );
    (repo, s1, s2)
}

#[test]
fn everything_sets_aside_ignored_files_in_the_way_of_a_file_or_a_folder() {
    let (repo, s1, _s2) = ignored_paths_in_the_way_of_a_save();

    let entry = bring_back_everything(&open(&repo), &s1, UnsavedPolicy::RequireClean)
        .unwrap()
        .expect("the ignored files were set aside");

    assert_eq!(read(&repo, "build"), "a file");
    assert_eq!(read(&repo, "logs/a.txt"), "a");
    assert_eq!(
        entry.files,
        vec!["build/out.js".to_owned(), "logs".to_owned()]
    );
    assert_eq!(
        kept_untracked(&repo, &entry.ref_name, "build/out.js"),
        "compiled"
    );
    assert_eq!(kept_untracked(&repo, &entry.ref_name, "logs"), "log text");
    let p = open(&repo);
    assert_eq!(list_recovery(&p).unwrap().len(), 1, "one entry for both");

    // And they come back.
    restore_recovery(&p, &entry.ref_name).unwrap();
    assert_eq!(read(&repo, "build/out.js"), "compiled");
    assert_eq!(read(&repo, "logs"), "log text");
}

#[test]
fn file_sets_aside_an_ignored_file_that_is_where_a_folder_is_needed() {
    let (repo, s1, _s2) = ignored_paths_in_the_way_of_a_save();
    let p = open(&repo);

    let entry = bring_back_file(&p, &Side::Save(s1), "logs/a.txt")
        .unwrap()
        .expect("the ignored file `logs` was set aside");

    assert_eq!(read(&repo, "logs/a.txt"), "a");
    assert_eq!(entry.files, vec!["logs".to_owned()]);
    assert_eq!(kept_untracked(&repo, &entry.ref_name, "logs"), "log text");
    assert_eq!(
        read(&repo, "build/out.js"),
        "compiled",
        "what is not in the way stays"
    );
}

#[test]
fn file_sets_aside_an_ignored_folder_that_is_where_a_file_is_needed() {
    let (repo, s1, _s2) = ignored_paths_in_the_way_of_a_save();
    let p = open(&repo);

    let entry = bring_back_file(&p, &Side::Save(s1), "build")
        .unwrap()
        .expect("the ignored folder `build` was set aside");

    assert_eq!(read(&repo, "build"), "a file");
    assert_eq!(entry.files, vec!["build/out.js".to_owned()]);
    assert_eq!(
        kept_untracked(&repo, &entry.ref_name, "build/out.js"),
        "compiled"
    );
    assert_eq!(
        read(&repo, "logs"),
        "log text",
        "not in the way, so it stays"
    );
}

#[test]
fn deleting_a_file_leaves_ignored_files_around_it_alone() {
    let (repo, _s1, s2) = ignored_paths_in_the_way_of_a_save();
    let p = open(&repo);

    // `logs/a.txt` is not in the second save, so bringing it back deletes it: nothing there to
    // delete, and nothing is needed from the ignored file `logs`.
    let set_aside = bring_back_file(&p, &Side::Save(s2), "logs/a.txt").unwrap();

    assert!(set_aside.is_none());
    assert_eq!(read(&repo, "logs"), "log text");
    assert_eq!(read(&repo, "build/out.js"), "compiled");
}

#[test]
fn everything_sets_aside_a_file_that_only_an_unsaved_ignore_edit_ignores() {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    repo.write("secret.txt", "old secret");
    let s1 = SaveId(repo.commit_all("s1"));
    repo.git(&["rm", "-q", "secret.txt"]);
    let s2 = SaveId(repo.commit_all("s2"));
    // Unsaved: the file is ignored by a new `.gitignore`, which is itself a change.
    repo.write("secret.txt", "CURRENT secret");
    repo.write(".gitignore", "secret.txt\n");
    let p = open(&repo);
    assert_eq!(
        snapshot(&p).changes,
        vec![change(".gitignore", ChangeKind::New)]
    );

    let entry = bring_back_everything(&p, &s1, UnsavedPolicy::MoveToRecovery)
        .unwrap()
        .expect("something was set aside");

    // Throwing the `.gitignore` away first would make `secret.txt` an ordinary new file that
    // nothing sets aside.
    assert_eq!(
        entry.files,
        vec![".gitignore".to_owned()],
        "the most recent"
    );
    assert_eq!(read(&repo, "secret.txt"), "old secret");
    let kept: Vec<String> = list_recovery(&p)
        .unwrap()
        .into_iter()
        .filter(|e| e.files == vec!["secret.txt".to_owned()])
        .map(|e| kept_untracked(&repo, &e.ref_name, "secret.txt"))
        .collect();
    assert_eq!(kept, vec!["CURRENT secret".to_owned()]);
    assert_eq!(head(&repo), s2.0);
}

#[test]
fn a_long_list_of_ignored_files_is_set_aside_in_one_entry() {
    // Together the names are far longer than a command line can be (32 KB on Windows): about 58 KB.
    const FILES: usize = 500;
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    for n in 0..FILES {
        repo.write(&long_package_file(n), format!("old {n}"));
    }
    let s1 = SaveId(repo.commit_all("s1"));
    let p = open(&repo);
    // Stopped saving: out of the index, and ignored by a rule that no save carries along.
    rm_cached(p.repo(), "node_modules", true).unwrap();
    repo.write(".git/info/exclude", "node_modules/\n");
    repo.commit_all("s2");
    repo.write(&long_package_file(0), "current first");
    repo.write(&long_package_file(FILES - 1), "current last");
    assert!(snapshot(&p).changes.is_empty(), "ignored, so not changes");

    let entry = bring_back_everything(&p, &s1, UnsavedPolicy::RequireClean)
        .unwrap()
        .expect("the ignored files were set aside");

    assert_eq!(entry.files.len(), FILES);
    assert_eq!(read(&repo, &long_package_file(0)), "old 0");
    assert_eq!(
        kept_untracked(&repo, &entry.ref_name, &long_package_file(0)),
        "current first"
    );
    assert_eq!(
        list_recovery(&p).unwrap().len(),
        1,
        "one entry, no leftovers"
    );

    restore_recovery(&p, &entry.ref_name).unwrap();
    assert_eq!(read(&repo, &long_package_file(0)), "current first");
    assert_eq!(read(&repo, &long_package_file(FILES - 1)), "current last");
    assert_eq!(read(&repo, &long_package_file(1)), "old 1");
}

#[test]
fn a_read_only_ignored_file_is_set_aside_and_removed() {
    let (repo, _s1, s2) = stopped_saving_env();
    let mut permissions = fs::metadata(repo.path().join(".env"))
        .unwrap()
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(repo.path().join(".env"), permissions).unwrap();
    let p = open(&repo);

    let entry = bring_back_file(&p, &Side::Save(s2), ".env")
        .unwrap()
        .expect("the ignored .env was set aside");

    assert!(!exists(&repo, ".env"));
    assert_eq!(
        kept_untracked(&repo, &entry.ref_name, ".env"),
        "CURRENT secret"
    );
}

/// The ignored file is spelled `.ENV` on disk and `.env` in the save, and the project treats
/// both as one file.
#[cfg(windows)]
#[test]
fn file_finds_an_ignored_file_spelled_in_another_case() {
    let (repo, _s1, s2) = stopped_saving_env();
    let p = open(&repo);
    repo.git(&["config", "core.ignorecase", "true"]);
    repo.remove(".env");
    repo.write(".ENV", "CURRENT secret");
    assert!(
        snapshot(&p).changes.is_empty(),
        "`.ENV` is ignored by the `.env` rule"
    );

    let from_before = bring_back_file(&p, &Side::Before(s2), ".env")
        .unwrap()
        .expect("the ignored .ENV was set aside");

    assert_eq!(from_before.files, vec![".ENV".to_owned()]);
    assert_eq!(
        kept_untracked(&repo, &from_before.ref_name, ".ENV"),
        "CURRENT secret"
    );
    assert_eq!(read(&repo, ".env"), "old secret");
}

/// A file of a package with a long name, as under `node_modules`.
fn long_package_file(n: usize) -> String {
    format!(
        "node_modules/{}{n:04}/index.js",
        "long-package-name-".repeat(5)
    )
}

#[test]
fn folders_left_empty_by_setting_ignored_files_aside_are_removed() {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    repo.write("cache/deep/x.bin", "old");
    repo.commit_all("s1");
    let p = open(&repo);
    rm_cached(p.repo(), "cache", true).unwrap();
    repo.write(".gitignore", "cache/\n");
    repo.write("cache/other/y.bin", "other");
    let s2 = SaveId(repo.commit_all("s2"));
    repo.write("cache/deep/x.bin", "CURRENT");

    let entry = bring_back_file(&p, &Side::Save(s2), "cache/deep/x.bin")
        .unwrap()
        .expect("the ignored file was set aside");

    assert_eq!(
        kept_untracked(&repo, &entry.ref_name, "cache/deep/x.bin"),
        "CURRENT"
    );
    assert!(!exists(&repo, "cache/deep"), "left empty, so removed");
    assert_eq!(
        read(&repo, "cache/other/y.bin"),
        "other",
        "not empty, so kept"
    );
}
