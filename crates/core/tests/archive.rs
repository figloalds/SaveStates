use std::path::PathBuf;

use savestates_core::{
    ArchiveError, DELETED_PREFIX, OpenOutcome, Project, Timeline, archive_timeline,
    delete_timeline, list_archived, list_deleted, list_timelines, open_project, read_timelines,
    rename_timeline, restore_deleted, unarchive_timeline,
};
use savestates_git::{config_get, config_get_regexp, refs_with_prefix};
use testkit::TestRepo;

fn open(repo: &TestRepo) -> Project {
    match open_project(&repo.exe(), repo.path()).unwrap() {
        OpenOutcome::Opened(project) => project,
        other => panic!("expected Opened, got {other:?}"),
    }
}

fn save(repo: &TestRepo, file: &str, msg: &str) -> String {
    repo.write(file, msg);
    repo.commit_all(msg)
}

/// `main` with one save, and `savestates/x` with `own` saves of its own; back on `main`.
fn with_x(own: usize) -> TestRepo {
    let repo = TestRepo::new();
    save(&repo, "a", "one");
    repo.git(&["switch", "-q", "-c", "savestates/x"]);
    for i in 0..own {
        save(&repo, &format!("x{i}"), &format!("x{i}"));
    }
    repo.git(&["switch", "-q", "main"]);
    repo
}

fn timeline(p: &Project, branch: &str) -> Option<Timeline> {
    list_timelines(p)
        .unwrap()
        .into_iter()
        .find(|t| t.branch == branch)
}

/// Checks `savestates/x` out in a second folder, so it is "elsewhere".
fn check_out_elsewhere(repo: &TestRepo) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "savestates-archive-wt-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    repo.git(&[
        "worktree",
        "add",
        "-q",
        dir.to_str().unwrap(),
        "savestates/x",
    ]);
    dir
}

#[test]
fn renaming_sets_and_clears_the_display_name() {
    let repo = TestRepo::new();
    save(&repo, "a", "one");
    repo.git(&["branch", "develop"]);
    let p = open(&repo);
    rename_timeline(&p, "develop", "  Versão do Claude ").unwrap();
    assert_eq!(
        timeline(&p, "develop").unwrap().display_name.as_deref(),
        Some("Versão do Claude")
    );
    rename_timeline(&p, "develop", "   ").unwrap();
    assert_eq!(timeline(&p, "develop").unwrap().display_name, None);
}

#[test]
fn archive_hides_and_unarchive_shows() {
    let repo = with_x(1);
    let p = open(&repo);
    archive_timeline(&p, "savestates/x").unwrap();
    let view = read_timelines(&p, 10).unwrap();
    assert!(
        view.timelines
            .iter()
            .find(|t| t.branch == "savestates/x")
            .unwrap()
            .archived
    );
    unarchive_timeline(&p, "savestates/x").unwrap();
    assert!(!timeline(&p, "savestates/x").unwrap().archived);
}

#[test]
fn archive_refuses_main_current_and_elsewhere() {
    let repo = with_x(1);
    let p = open(&repo);
    assert!(matches!(
        archive_timeline(&p, "main"),
        Err(ArchiveError::MainLine)
    ));
    repo.git(&["switch", "-q", "savestates/x"]);
    assert!(matches!(
        archive_timeline(&p, "savestates/x"),
        Err(ArchiveError::Current)
    ));
    repo.git(&["switch", "-q", "main"]);
    check_out_elsewhere(&repo);
    assert!(matches!(
        archive_timeline(&p, "savestates/x"),
        Err(ArchiveError::Elsewhere(_))
    ));
    assert!(matches!(
        archive_timeline(&p, "nope"),
        Err(ArchiveError::NotFound)
    ));
}

#[test]
fn archived_timelines_list_own_saves_and_last_save() {
    let repo = with_x(3);
    let tip = repo.git(&["rev-parse", "savestates/x"]).trim().to_owned();
    let p = open(&repo);
    assert!(list_archived(&p).unwrap().is_empty());
    archive_timeline(&p, "savestates/x").unwrap();
    let archived = list_archived(&p).unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].branch, "savestates/x");
    assert_eq!(archived[0].own_saves, 3);
    assert_eq!(archived[0].last_save.id.0, tip);
    assert_eq!(archived[0].last_save.name, "x2");
}

#[test]
fn delete_keeps_a_recoverable_copy() {
    let repo = with_x(2);
    let tip = repo.git(&["rev-parse", "savestates/x"]).trim().to_owned();
    let p = open(&repo);
    rename_timeline(&p, "savestates/x", "Versão com menu").unwrap();
    archive_timeline(&p, "savestates/x").unwrap();
    let gone = delete_timeline(&p, "savestates/x").unwrap();
    assert!(gone.ref_name.starts_with(DELETED_PREFIX));
    assert!(timeline(&p, "savestates/x").is_none());

    let deleted = list_deleted(&p).unwrap();
    assert_eq!(deleted, [gone]);
    assert_eq!(deleted[0].branch, "savestates/x");
    assert_eq!(deleted[0].display_name.as_deref(), Some("Versão com menu"));

    let used = restore_deleted(&p, &deleted[0].ref_name).unwrap();
    assert_eq!(used, "savestates/x");
    let t = timeline(&p, "savestates/x").unwrap();
    assert_eq!(t.tip.0, tip);
    assert_eq!(t.display_name.as_deref(), Some("Versão com menu"));
    assert!(t.archived);
    assert!(list_deleted(&p).unwrap().is_empty());
    assert!(
        refs_with_prefix(p.repo(), DELETED_PREFIX)
            .unwrap()
            .is_empty()
    );
    assert!(
        config_get_regexp(p.repo(), r"^savestates-deleted\.")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        config_get(p.repo(), "branch.savestates/x.description")
            .unwrap()
            .as_deref(),
        Some("Versão com menu")
    );
}

#[test]
fn restoring_picks_a_free_name() {
    let repo = with_x(1);
    let p = open(&repo);
    delete_timeline(&p, "savestates/x").unwrap();
    repo.git(&["branch", "savestates/x"]);
    let deleted = list_deleted(&p).unwrap();
    assert_eq!(
        restore_deleted(&p, &deleted[0].ref_name).unwrap(),
        "savestates/x-2"
    );
    assert!(timeline(&p, "savestates/x-2").is_some());
    assert!(timeline(&p, "savestates/x").is_some());
}

#[test]
fn delete_refuses_main_and_elsewhere() {
    let repo = with_x(1);
    let p = open(&repo);
    assert!(matches!(
        delete_timeline(&p, "main"),
        Err(ArchiveError::MainLine)
    ));
    check_out_elsewhere(&repo);
    assert!(matches!(
        delete_timeline(&p, "savestates/x"),
        Err(ArchiveError::Elsewhere(_))
    ));
    assert!(timeline(&p, "savestates/x").is_some());
    assert!(list_deleted(&p).unwrap().is_empty());
}

/// Another git program holds the branch (its `.lock` file exists), so git cannot delete it
/// after the copy was made: the copy and its settings go again, and the timeline stays as it
/// was, with nothing listed as deleted.
#[test]
fn a_delete_git_refuses_leaves_no_copy_behind() {
    let repo = with_x(1);
    let p = open(&repo);
    rename_timeline(&p, "savestates/x", "Menu").unwrap();
    let lock = repo
        .path()
        .join(".git")
        .join("refs")
        .join("heads")
        .join("savestates")
        .join("x.lock");
    std::fs::write(&lock, "").unwrap();
    let result = delete_timeline(&p, "savestates/x");
    std::fs::remove_file(&lock).unwrap();

    assert!(matches!(result, Err(ArchiveError::Core(_))), "{result:?}");
    assert!(timeline(&p, "savestates/x").is_some());
    assert!(list_deleted(&p).unwrap().is_empty());
    assert!(
        refs_with_prefix(p.repo(), DELETED_PREFIX)
            .unwrap()
            .is_empty()
    );
    assert!(
        config_get_regexp(p.repo(), r"^savestates-deleted\.")
            .unwrap()
            .is_empty()
    );
}
