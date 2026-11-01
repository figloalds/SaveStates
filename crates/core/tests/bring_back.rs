use std::fs;

use savestates_core::{
    BringBackError, Change, ChangeKind, CoreError, OpenOutcome, Project, SaveId, Side,
    UnsavedPolicy, bring_back_everything, bring_back_file, list_recovery, open_project,
    restore_recovery, snapshot,
};
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
