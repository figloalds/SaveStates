use std::fs;
use std::path::Path;

use savestates_core::{
    Blocker, Change, ChangeKind, GITIGNORE_TEMPLATE, OpenOutcome, Project, RepoCondition, SaveId,
    open_project, snapshot, start_saving_folder,
};
use savestates_git::RunOpts;
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

/// Runs a git command that is expected to stop on a conflict (non-zero exit).
fn run_expecting_conflict(repo: &TestRepo, args: &[&str]) {
    let result = repo.repo().run(args, &RunOpts::default());
    assert!(
        result.is_err(),
        "git {args:?} should have stopped on a conflict"
    );
}

fn blockers_of(repo: &TestRepo) -> Vec<Blocker> {
    match snapshot(&open(repo)).condition {
        RepoCondition::ReadOnly(blockers) => blockers,
        other => panic!("expected ReadOnly, got {other:?}"),
    }
}

/// `main` and `topic` both rewrite `f.txt` after a common base commit. Left on `main`.
fn diverged_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.write("f.txt", "base\n");
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "topic"]);
    repo.write("f.txt", "topic\n");
    repo.commit_all("topic");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("f.txt", "main\n");
    repo.commit_all("main");
    repo
}

#[test]
fn opens_from_subfolder() {
    let repo = TestRepo::new();
    repo.write("src/deep/x.txt", "x");

    let outcome = open_project(&repo.exe(), &repo.path().join("src").join("deep")).unwrap();

    let OpenOutcome::Opened(project) = outcome else {
        panic!("expected Opened, got {outcome:?}");
    };
    assert_eq!(
        dunce::canonicalize(project.root()).unwrap(),
        dunce::canonicalize(repo.path()).unwrap()
    );
}

#[test]
fn non_repo_and_missing() {
    let repo = TestRepo::empty_dir();

    let not_a_repo = open_project(&repo.exe(), repo.path()).unwrap();
    let missing = open_project(&repo.exe(), &repo.path().join("nope")).unwrap();

    assert!(
        matches!(not_a_repo, OpenOutcome::NotARepo),
        "{not_a_repo:?}"
    );
    assert!(matches!(missing, OpenOutcome::Missing), "{missing:?}");
}

#[test]
fn start_saving_writes_template_once() {
    let fresh = TestRepo::empty_dir();
    let project = start_saving_folder(&fresh.exe(), fresh.path()).unwrap();

    assert_eq!(
        fs::read_to_string(fresh.path().join(".gitignore")).unwrap(),
        GITIGNORE_TEMPLATE
    );
    let head = project
        .repo()
        .run(&["symbolic-ref", "--short", "HEAD"], &RunOpts::default())
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&head).trim(), "main");
    assert_eq!(
        dunce::canonicalize(project.root()).unwrap(),
        dunce::canonicalize(fresh.path()).unwrap()
    );

    let custom = TestRepo::empty_dir();
    custom.write(".gitignore", "custom\n");
    start_saving_folder(&custom.exe(), custom.path()).unwrap();
    assert_eq!(
        fs::read_to_string(custom.path().join(".gitignore")).unwrap(),
        "custom\n"
    );
}

#[test]
fn gitignore_template_is_the_specified_text() {
    let expected = [
        "# OS",
        ".DS_Store",
        "Thumbs.db",
        "desktop.ini",
        "# Dependencies and build output",
        "node_modules/",
        "dist/",
        "build/",
        "out/",
        ".next/",
        "target/",
        "__pycache__/",
        ".venv/",
        "venv/",
        "# Secrets",
        ".env",
        ".env.*",
        "!.env.example",
        "# Editors and logs",
        "*.log",
        ".idea/",
        ".vs/",
    ];
    assert_eq!(GITIGNORE_TEMPLATE, format!("{}\n", expected.join("\n")));
}

#[test]
fn empty_repo_snapshot() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a");

    let snap = snapshot(&open(&repo));

    assert_eq!(snap.condition, RepoCondition::Normal);
    assert_eq!(snap.head, None);
    assert_eq!(snap.timeline_name.as_deref(), Some("main"));
    assert_eq!(snap.changes, vec![change("a.txt", ChangeKind::New)]);
    assert!(!snap.other_timelines);
}

#[test]
fn committed_repo_snapshot_reports_head() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a");
    let oid = repo.commit_all("first");

    let snap = snapshot(&open(&repo));

    assert_eq!(snap.condition, RepoCondition::Normal);
    assert_eq!(snap.head, Some(SaveId(oid)));
    assert!(snap.changes.is_empty());
}

#[test]
fn change_kinds() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.write("gone.txt", "gone\n");
    repo.write("same.txt", "same\n");
    repo.commit_all("base");

    repo.write("a.txt", "changed\n");
    repo.remove("gone.txt");
    repo.write("new.txt", "new\n");
    let snap = snapshot(&open(&repo));
    assert_eq!(snap.condition, RepoCondition::Normal);
    assert_eq!(
        snap.changes,
        vec![
            change("a.txt", ChangeKind::Changed),
            change("gone.txt", ChangeKind::Deleted),
            change("new.txt", ChangeKind::New),
        ]
    );

    let renamed = TestRepo::new();
    renamed.write("a.txt", "some content\n");
    renamed.commit_all("base");
    renamed.git(&["mv", "a.txt", "b.txt"]);
    let snap = snapshot(&open(&renamed));
    assert_eq!(
        snap.changes,
        vec![change(
            "b.txt",
            ChangeKind::Renamed {
                from: "a.txt".to_owned()
            }
        )]
    );
}

#[test]
fn staged_changes_map_to_the_same_kinds() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a\n");
    repo.write("gone.txt", "gone\n");
    repo.commit_all("base");

    repo.write("a.txt", "changed\n");
    repo.remove("gone.txt");
    repo.write("added.txt", "added\n");
    repo.git(&["add", "-A"]);

    let snap = snapshot(&open(&repo));

    assert_eq!(
        snap.changes,
        vec![
            change("a.txt", ChangeKind::Changed),
            change("added.txt", ChangeKind::New),
            change("gone.txt", ChangeKind::Deleted),
        ]
    );
}

#[test]
fn conditions() {
    // Conflicting merge: Merge and Conflicts.
    let repo = diverged_repo();
    run_expecting_conflict(&repo, &["merge", "topic"]);
    let blockers = blockers_of(&repo);
    assert!(blockers.contains(&Blocker::Merge), "{blockers:?}");
    assert!(blockers.contains(&Blocker::Conflicts), "{blockers:?}");

    // Detached HEAD.
    let repo = TestRepo::new();
    repo.write("a.txt", "a");
    repo.commit_all("first");
    repo.git(&["checkout", "-q", "--detach"]);
    assert_eq!(
        snapshot(&open(&repo)).condition,
        RepoCondition::ReadOnly(vec![Blocker::Detached])
    );

    // Conflicting rebase.
    let repo = diverged_repo();
    repo.git(&["checkout", "-q", "topic"]);
    run_expecting_conflict(&repo, &["rebase", "main"]);
    let blockers = blockers_of(&repo);
    assert!(blockers.contains(&Blocker::Rebase), "{blockers:?}");
    assert!(blockers.contains(&Blocker::Conflicts), "{blockers:?}");

    // Conflicting cherry-pick.
    let repo = diverged_repo();
    run_expecting_conflict(&repo, &["cherry-pick", "topic"]);
    let blockers = blockers_of(&repo);
    assert!(blockers.contains(&Blocker::CherryPick), "{blockers:?}");
    assert!(blockers.contains(&Blocker::Conflicts), "{blockers:?}");

    // Conflicting revert.
    let repo = TestRepo::new();
    for n in 1..=3 {
        repo.write("f.txt", format!("{n}\n"));
        repo.commit_all(&format!("v{n}"));
    }
    run_expecting_conflict(&repo, &["revert", "--no-edit", "HEAD~1"]);
    let blockers = blockers_of(&repo);
    assert!(blockers.contains(&Blocker::Revert), "{blockers:?}");
    assert!(blockers.contains(&Blocker::Conflicts), "{blockers:?}");

    // Bisect in progress.
    let repo = TestRepo::new();
    repo.write("a.txt", "a");
    repo.commit_all("first");
    repo.git(&["bisect", "start"]);
    assert!(blockers_of(&repo).contains(&Blocker::Bisect));

    // Bare repository.
    let bare = TestRepo::empty_dir();
    bare.git(&["init", "-q", "--bare"]);
    assert_eq!(
        snapshot(&open(&bare)).condition,
        RepoCondition::ReadOnly(vec![Blocker::Bare])
    );

    // A missing HEAD commit object makes the repository unreadable.
    let repo = TestRepo::new();
    repo.write("a.txt", "a");
    let oid = repo.commit_all("first");
    delete_loose_object(repo.path(), &oid);
    let snap = snapshot(&open(&repo));
    let RepoCondition::Broken { details } = &snap.condition else {
        panic!("expected Broken, got {:?}", snap.condition);
    };
    assert!(!details.is_empty());
}

#[test]
fn other_timelines_flag() {
    let repo = TestRepo::new();
    repo.write("a.txt", "a");
    repo.commit_all("first");
    assert!(!snapshot(&open(&repo)).other_timelines);

    repo.git(&["branch", "other"]);

    assert!(snapshot(&open(&repo)).other_timelines);
}

/// Loose objects are read-only on Windows, so the attribute is cleared before deleting.
#[allow(clippy::permissions_set_readonly_false)]
fn delete_loose_object(root: &Path, oid: &str) {
    let object = root
        .join(".git")
        .join("objects")
        .join(&oid[..2])
        .join(&oid[2..]);
    let mut permissions = fs::metadata(&object).unwrap().permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&object, permissions).unwrap();
    fs::remove_file(&object).unwrap();
}

#[test]
fn the_timelines_key_follows_refs_names_and_flags() {
    let repo = TestRepo::new();
    repo.write("a", "1");
    repo.commit_all("one");
    let p = open(&repo);
    let key = || snapshot(&p).timelines_key;
    let mut seen = vec![key()];
    let changed = |what: &str, seen: &mut Vec<String>| {
        let k = key();
        assert_ne!(&k, seen.last().unwrap(), "{what} should change the key");
        seen.push(k);
    };

    repo.git(&["branch", "other"]);
    changed("a new branch", &mut seen);
    repo.write("b", "2");
    repo.commit_all("two");
    changed("a moved branch", &mut seen);
    repo.git(&["config", "branch.other.description", "Named"]);
    changed("a description", &mut seen);
    repo.git(&["config", "branch.other.savestates-archived", "true"]);
    changed("the archived flag", &mut seen);
    repo.git(&["switch", "-q", "other"]);
    changed("a switch", &mut seen);

    let before = key();
    repo.write("c", "unsaved edit");
    assert_eq!(key(), before, "editing a file must not change the key");
}
