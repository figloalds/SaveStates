use std::fs;

use savestates_core::{
    Change, ChangeKind, CoreError, OpenOutcome, Project, RECOVERY_MAX_AGE_DAYS, RECOVERY_PREFIX,
    RepoCondition, Scope, list_recovery, open_project, prune_recovery, restore_recovery, snapshot,
    throw_away,
};
use savestates_git::{EntryKind, delete_ref, refs_with_prefix, rm_cached, status, update_ref};
use testkit::TestRepo;

const DAY: i64 = 86_400;

fn open(repo: &TestRepo) -> Project {
    match open_project(&repo.exe(), repo.path()).unwrap() {
        OpenOutcome::Opened(project) => project,
        other => panic!("expected Opened, got {other:?}"),
    }
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
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

/// A project with `a.txt`, `c.txt` and `u.txt` saved (all `v1`) and a stash of the user's own
/// holding an unrelated `u.txt` edit. The folder is clean.
fn project_with_a_user_stash() -> TestRepo {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    repo.write("c.txt", "v1");
    repo.write("u.txt", "v1");
    repo.commit_all("s1");
    repo.write("u.txt", "the user's own work");
    repo.git(&["stash", "push", "-m", "mine"]);
    repo
}

fn stash_list(repo: &TestRepo) -> Vec<String> {
    repo.git(&["stash", "list"])
        .lines()
        .map(str::to_owned)
        .collect()
}

fn recovery_refs(p: &Project) -> Vec<(String, String)> {
    refs_with_prefix(p.repo(), RECOVERY_PREFIX).unwrap()
}

#[test]
fn throw_away_all_snapshots_and_cleans() {
    let repo = project_with_a_user_stash();
    repo.write("a.txt", "changed");
    repo.write("b.txt", "new");
    let p = open(&repo);

    let entry = throw_away(&p, &Scope::All)
        .unwrap()
        .expect("there were changes to throw away");

    assert!(snapshot(&p).changes.is_empty());
    assert_eq!(read(&repo, "a.txt"), "v1");
    assert!(!exists(&repo, "b.txt"));
    assert_eq!(repo.git(&["status", "--porcelain"]), "");

    assert_eq!(entry.files, strings(&["a.txt", "b.txt"]));
    assert!(entry.ref_name.starts_with(RECOVERY_PREFIX));
    assert!(
        recovery_refs(&p)
            .iter()
            .any(|(name, _)| name == &entry.ref_name)
    );

    let stashes = stash_list(&repo);
    assert_eq!(stashes.len(), 1, "only the user's own stash: {stashes:?}");
    assert!(stashes[0].contains("mine"), "{stashes:?}");
}

#[test]
fn throw_away_one_file() {
    let repo = project_with_a_user_stash();
    repo.write("a.txt", "changed");
    repo.write("c.txt", "also changed");
    let p = open(&repo);

    let entry = throw_away(&p, &Scope::File("a.txt".to_owned()))
        .unwrap()
        .expect("a.txt had changes");

    assert_eq!(read(&repo, "a.txt"), "v1");
    assert_eq!(read(&repo, "c.txt"), "also changed");
    assert_eq!(
        snapshot(&p).changes,
        vec![change("c.txt", ChangeKind::Changed)]
    );
    assert_eq!(entry.files, strings(&["a.txt"]));
    assert_eq!(stash_list(&repo).len(), 1);
}

#[test]
fn throw_away_one_file_matches_its_name_literally() {
    // As a glob, `[id].js` would also match `i.js` and `d.js`.
    let repo = TestRepo::new();
    repo.write("src/[id].js", "v1");
    repo.write("src/i.js", "v1");
    repo.commit_all("s1");
    repo.write("src/[id].js", "changed");
    repo.write("src/i.js", "changed");
    repo.write("src/[new].js", "new");
    repo.write("src/n.js", "new");
    let p = open(&repo);

    let tracked = throw_away(&p, &Scope::File("src/[id].js".to_owned()))
        .unwrap()
        .unwrap();
    assert_eq!(tracked.files, strings(&["src/[id].js"]));
    assert_eq!(read(&repo, "src/[id].js"), "v1");
    assert_eq!(read(&repo, "src/i.js"), "changed");

    let untracked = throw_away(&p, &Scope::File("src/[new].js".to_owned()))
        .unwrap()
        .unwrap();
    assert_eq!(untracked.files, strings(&["src/[new].js"]));
    assert!(!exists(&repo, "src/[new].js"));
    assert_eq!(read(&repo, "src/n.js"), "new");
}

#[test]
fn throw_away_accented_path() {
    let repo = TestRepo::new();
    repo.write("Área/Ação.md", "v1");
    repo.commit_all("s1");
    repo.write("Área/Ação.md", "changed");
    repo.write("Área/Nova ação.md", "new");
    let p = open(&repo);

    let entry = throw_away(&p, &Scope::All).unwrap().unwrap();

    assert_eq!(entry.files, strings(&["Área/Ação.md", "Área/Nova ação.md"]));
    assert_eq!(read(&repo, "Área/Ação.md"), "v1");
    assert!(!exists(&repo, "Área/Nova ação.md"));
}

#[test]
fn throw_away_nothing() {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    repo.commit_all("s1");
    let p = open(&repo);

    assert!(throw_away(&p, &Scope::All).unwrap().is_none());
    assert!(
        throw_away(&p, &Scope::File("a.txt".to_owned()))
            .unwrap()
            .is_none()
    );
    assert!(recovery_refs(&p).is_empty());
}

#[test]
fn throw_away_nothing_leaves_the_users_stash_alone() {
    // `git stash push -q` says nothing when there is nothing to stash, and the user's own
    // entry is then still the newest one: it must not be mistaken for ours.
    let repo = project_with_a_user_stash();
    let p = open(&repo);

    assert!(throw_away(&p, &Scope::All).unwrap().is_none());
    assert!(
        throw_away(&p, &Scope::File("a.txt".to_owned()))
            .unwrap()
            .is_none()
    );

    assert!(recovery_refs(&p).is_empty());
    let stashes = stash_list(&repo);
    assert_eq!(stashes.len(), 1, "{stashes:?}");
    assert!(stashes[0].contains("mine"), "{stashes:?}");
}

#[test]
fn throw_away_empty_repo() {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    let p = open(&repo);

    assert!(matches!(
        throw_away(&p, &Scope::All),
        Err(CoreError::NoSavesYet)
    ));
    assert_eq!(read(&repo, "a.txt"), "v1", "nothing was touched");
}

#[test]
fn throw_away_works_without_a_git_identity() {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    repo.commit_all("s1");
    repo.git(&["config", "--unset", "user.name"]);
    repo.git(&["config", "--unset", "user.email"]);
    repo.write("a.txt", "changed");
    let p = open(&repo);

    let entry = throw_away(&p, &Scope::All).unwrap().unwrap();

    let author = repo.git(&["log", "-1", "--format=%an <%ae>", &entry.ref_name]);
    assert_eq!(author.trim(), "SaveStates <savestates@localhost>");
}

#[test]
fn recovery_names_are_unique_and_listed_newest_first() {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    repo.commit_all("s1");
    let p = open(&repo);

    let mut made = Vec::new();
    for round in 0..3 {
        repo.write("a.txt", format!("changed {round}"));
        made.push(throw_away(&p, &Scope::All).unwrap().unwrap());
    }

    let listed = list_recovery(&p).unwrap();
    let mut expected = made.clone();
    expected.reverse();
    assert_eq!(listed, expected);
    let mut names: Vec<_> = listed.iter().map(|e| e.ref_name.clone()).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), 3);
}

#[test]
fn list_ignores_refs_that_are_not_ours() {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    let head = repo.commit_all("s1");
    let p = open(&repo);
    update_ref(
        p.repo(),
        &format!("{RECOVERY_PREFIX}not-a-timestamp"),
        &head,
    )
    .unwrap();
    // A commit that is not shaped like a stash still lists, with no files.
    update_ref(p.repo(), &format!("{RECOVERY_PREFIX}1000-0"), &head).unwrap();

    let listed = list_recovery(&p).unwrap();

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].created_unix, 1000);
    assert!(listed[0].files.is_empty());
}

#[test]
fn restore_recovery_round_trip() {
    let repo = project_with_a_user_stash();
    repo.write("a.txt", "changed");
    repo.write("b.txt", "new");
    let p = open(&repo);
    let entry = throw_away(&p, &Scope::All).unwrap().unwrap();

    let moved = restore_recovery(&p, &entry.ref_name).unwrap();

    assert!(
        moved.is_none(),
        "the folder was clean, nothing to set aside"
    );
    assert_eq!(read(&repo, "a.txt"), "changed");
    assert_eq!(read(&repo, "b.txt"), "new");
    assert!(recovery_refs(&p).is_empty());
    assert!(list_recovery(&p).unwrap().is_empty());
    assert_eq!(
        snapshot(&p).changes,
        vec![
            change("a.txt", ChangeKind::Changed),
            change("b.txt", ChangeKind::New),
        ]
    );
    assert_eq!(stash_list(&repo).len(), 1, "the user's stash is untouched");
}

#[test]
fn restore_recovery_of_one_file_with_a_special_name() {
    let repo = TestRepo::new();
    repo.write("Área/[id].md", "v1");
    repo.write("Área/i.md", "v1");
    repo.commit_all("s1");
    repo.write("Área/[id].md", "changed");
    repo.write("Área/i.md", "changed");
    let p = open(&repo);
    let entry = throw_away(&p, &Scope::File("Área/[id].md".to_owned()))
        .unwrap()
        .unwrap();

    // `i.md` is still changed, so it is set aside by the restore.
    let moved = restore_recovery(&p, &entry.ref_name).unwrap().unwrap();

    assert_eq!(moved.files, strings(&["Área/i.md"]));
    assert_eq!(read(&repo, "Área/[id].md"), "changed");
    assert_eq!(read(&repo, "Área/i.md"), "v1");
}

#[test]
fn restore_with_unsaved_moves_them_first() {
    let repo = project_with_a_user_stash();
    repo.write("a.txt", "changed");
    repo.write("b.txt", "new");
    let p = open(&repo);
    let entry1 = throw_away(&p, &Scope::All).unwrap().unwrap();
    repo.write("c.txt", "newer work");

    let entry2 = restore_recovery(&p, &entry1.ref_name)
        .unwrap()
        .expect("the unsaved change was set aside");

    assert_eq!(entry2.files, strings(&["c.txt"]));
    assert_ne!(entry2.ref_name, entry1.ref_name);
    assert_eq!(read(&repo, "a.txt"), "changed");
    assert_eq!(read(&repo, "b.txt"), "new");
    assert_eq!(
        read(&repo, "c.txt"),
        "v1",
        "c.txt went to the recovery area"
    );
    let refs = recovery_refs(&p);
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].0, entry2.ref_name);
}

#[test]
fn restore_unknown_ref_is_not_found() {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    repo.commit_all("s1");
    repo.write("a.txt", "unsaved");
    let p = open(&repo);
    let name = format!("{RECOVERY_PREFIX}1-0");

    match restore_recovery(&p, &name) {
        Err(CoreError::NotFound(missing)) => assert_eq!(missing, name),
        other => panic!("expected NotFound, got {other:?}"),
    }
    // Looking it up comes first: the unsaved change is not set aside for nothing.
    assert_eq!(read(&repo, "a.txt"), "unsaved");
    assert!(recovery_refs(&p).is_empty());
}

#[test]
fn restore_only_touches_recovery_refs() {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    repo.commit_all("s1");
    let p = open(&repo);

    assert!(matches!(
        restore_recovery(&p, "refs/heads/main"),
        Err(CoreError::NotFound(_))
    ));
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "main"
    );
}

#[test]
fn restore_conflict_keeps_the_entry_and_leaves_a_clean_folder() {
    let repo = TestRepo::new();
    repo.write("a.txt", "l1\nl2\nl3\n");
    repo.commit_all("s1");
    repo.write("a.txt", "l1\nmine\nl3\n");
    let p = open(&repo);
    let entry = throw_away(&p, &Scope::All).unwrap().unwrap();
    // The same line is saved differently in the meantime.
    repo.write("a.txt", "l1\ntheirs\nl3\n");
    let head = repo.commit_all("s2");

    let result = restore_recovery(&p, &entry.ref_name);

    assert!(
        matches!(result, Err(CoreError::RecoveryConflict)),
        "{result:?}"
    );
    assert_eq!(recovery_refs(&p).len(), 1, "the entry is kept");
    assert_eq!(read(&repo, "a.txt"), "l1\ntheirs\nl3\n");
    let after = snapshot(&p);
    assert_eq!(after.condition, RepoCondition::Normal);
    assert!(after.changes.is_empty(), "{:?}", after.changes);
    assert_eq!(repo.git(&["rev-parse", "HEAD"]).trim(), head);
}

#[test]
fn prune_by_age() {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    repo.commit_all("s1");
    repo.write("a.txt", "changed");
    let p = open(&repo);
    let entry = throw_away(&p, &Scope::All).unwrap().unwrap();
    let oid = recovery_refs(&p)[0].1.clone();
    delete_ref(p.repo(), &entry.ref_name).unwrap();

    let now = 1_800_000_000;
    let old = format!("{RECOVERY_PREFIX}{}-0", now - 31 * DAY);
    let recent = format!("{RECOVERY_PREFIX}{}-0", now - 29 * DAY);
    let foreign = format!("{RECOVERY_PREFIX}something-else");
    for name in [&old, &recent, &foreign] {
        update_ref(p.repo(), name, &oid).unwrap();
    }

    assert_eq!(RECOVERY_MAX_AGE_DAYS, 30);
    assert_eq!(prune_recovery(&p, now).unwrap(), 1);

    let mut left: Vec<_> = recovery_refs(&p)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    left.sort();
    let mut expected = vec![recent, foreign];
    expected.sort();
    assert_eq!(
        left, expected,
        "only the entry older than 30 days is pruned"
    );
    assert_eq!(prune_recovery(&p, now).unwrap(), 0);
}

/// What the snapshot `ref_name` holds for the new or ignored file `path` (git keeps those in the
/// stash's third parent).
fn snapshot_of_untracked(repo: &TestRepo, ref_name: &str, path: &str) -> String {
    repo.git(&["show", &format!("{ref_name}^3:{path}")])
}

#[test]
fn restore_with_a_modify_delete_conflict_leaves_no_unmerged_files() {
    let repo = TestRepo::new();
    repo.write("foo.txt", "v1");
    repo.write("keep.txt", "v1");
    repo.commit_all("s1");
    repo.write("foo.txt", "edited");
    let p = open(&repo);
    let entry = throw_away(&p, &Scope::All).unwrap().unwrap();
    // The file is deleted and that is saved: applying the edit now is a modify/delete conflict.
    repo.remove("foo.txt");
    let head = repo.commit_all("s2");

    let result = restore_recovery(&p, &entry.ref_name);

    assert!(
        matches!(result, Err(CoreError::RecoveryConflict)),
        "{result:?}"
    );
    let unmerged: Vec<_> = status(p.repo())
        .unwrap()
        .entries
        .into_iter()
        .filter(|e| e.kind == EntryKind::Unmerged)
        .collect();
    assert!(unmerged.is_empty(), "{unmerged:?}");
    assert_eq!(snapshot(&p).condition, RepoCondition::Normal);
    assert_eq!(recovery_refs(&p).len(), 1, "the entry is kept");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]).trim(), head);
}

/// `config.json` is saved, edited and thrown away; then it is saved as no longer saved
/// (ignored and out of the index), and the folder holds a newer, ignored `config.json`.
fn thrown_away_edit_of_a_file_that_is_now_ignored() -> (TestRepo, String) {
    let repo = TestRepo::new();
    repo.write("config.json", "v1");
    repo.write("a.txt", "v1");
    repo.commit_all("s1");
    repo.write("config.json", "edited");
    let p = open(&repo);
    let entry = throw_away(&p, &Scope::All).unwrap().unwrap();
    repo.write(".gitignore", "config.json\n");
    rm_cached(p.repo(), "config.json", false).unwrap();
    repo.commit_all("s2");
    repo.write("config.json", "CURRENT");
    assert!(snapshot(&p).changes.is_empty(), "ignored, so not a change");
    (repo, entry.ref_name)
}

#[test]
fn restore_keeps_the_current_content_of_an_ignored_file_it_collides_with() {
    let (repo, ref_name) = thrown_away_edit_of_a_file_that_is_now_ignored();
    let p = open(&repo);

    // The saved deletion and the old edit conflict, but whatever happens, the file that is on
    // disk right now must have been put in Recovery first.
    let result = restore_recovery(&p, &ref_name);

    assert!(
        matches!(result, Err(CoreError::RecoveryConflict)),
        "{result:?}"
    );
    let others: Vec<_> = list_recovery(&p)
        .unwrap()
        .into_iter()
        .filter(|e| e.ref_name != ref_name)
        .collect();
    assert_eq!(others.len(), 1, "{others:?}");
    assert_eq!(others[0].files, strings(&["config.json"]));
    assert_eq!(
        snapshot_of_untracked(&repo, &others[0].ref_name, "config.json"),
        "CURRENT"
    );
}

#[test]
fn restore_sets_aside_an_ignored_file_before_writing_over_it() {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    repo.commit_all("s1");
    repo.write("notes.txt", "thrown away");
    let p = open(&repo);
    let entry = throw_away(&p, &Scope::All).unwrap().unwrap();
    // `notes.txt` is now an ignored file of the folder, with its own content.
    repo.write(".gitignore", "notes.txt\n");
    repo.commit_all("s2");
    repo.write("notes.txt", "CURRENT");
    assert!(snapshot(&p).changes.is_empty());

    let set_aside = restore_recovery(&p, &entry.ref_name)
        .unwrap()
        .expect("the ignored file was set aside");

    assert_eq!(read(&repo, "notes.txt"), "thrown away");
    assert_eq!(set_aside.files, strings(&["notes.txt"]));
    assert_eq!(
        snapshot_of_untracked(&repo, &set_aside.ref_name, "notes.txt"),
        "CURRENT"
    );
    assert_eq!(
        recovery_refs(&p).len(),
        1,
        "only the set-aside file remains"
    );
}

#[test]
fn restore_does_not_sweep_ignored_files_it_does_not_touch() {
    let repo = TestRepo::new();
    repo.write(".gitignore", "*.log\nnode_modules/\n");
    repo.write("a.txt", "v1");
    repo.commit_all("s1");
    repo.write("a.txt", "changed");
    let p = open(&repo);
    let entry = throw_away(&p, &Scope::All).unwrap().unwrap();
    repo.write("local.log", "log");
    repo.write("node_modules/pkg/index.js", "module");

    let set_aside = restore_recovery(&p, &entry.ref_name).unwrap();

    assert!(set_aside.is_none());
    assert_eq!(read(&repo, "local.log"), "log");
    assert_eq!(read(&repo, "node_modules/pkg/index.js"), "module");
    assert!(recovery_refs(&p).is_empty());
}

/// An untracked file held open without delete sharing: git removes the other new files, then
/// fails to remove this one.
#[cfg(windows)]
#[test]
fn a_throw_away_that_fails_part_way_is_still_in_recovery() {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 1;

    let repo = project_with_a_user_stash();
    repo.write("a.txt", "changed");
    repo.write("b.txt", "new");
    repo.write("locked.txt", "held open");
    let _held = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(repo.path().join("locked.txt"))
        .unwrap();
    let p = open(&repo);

    let result = throw_away(&p, &Scope::All);

    assert!(matches!(result, Err(CoreError::Git(_))), "{result:?}");
    let listed = list_recovery(&p).unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert!(listed[0].files.contains(&"b.txt".to_owned()), "{listed:?}");
    assert!(listed[0].files.contains(&"a.txt".to_owned()), "{listed:?}");
    let stashes = stash_list(&repo);
    assert_eq!(stashes.len(), 1, "only the user's own stash: {stashes:?}");
    assert!(stashes[0].contains("mine"), "{stashes:?}");
}
