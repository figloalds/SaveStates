use savestates_git::{
    branches_containing, config_get, create_branch, delete_branch, head_branch, head_commit,
    list_branches, on_first_parent_line, own_first_parent_saves, switch_new, switch_to,
};
use testkit::TestRepo;

/// main: m1; feature: f1, f2 from m1; main: m2; merge --no-ff feature -> m3.
/// Returns (repo, m1, f2, m2, m3).
fn merge_repo() -> (TestRepo, String, String, String, String) {
    let t = TestRepo::new();
    t.git(&["checkout", "-q", "-b", "main"]);
    t.write("m.txt", "1");
    let m1 = t.commit_all("m1");
    t.git(&["checkout", "-q", "-b", "feature"]);
    t.write("f.txt", "1");
    t.commit_all("f1");
    t.write("f.txt", "2");
    let f2 = t.commit_all("f2");
    t.git(&["checkout", "-q", "main"]);
    t.write("m.txt", "2");
    let m2 = t.commit_all("m2");
    t.git(&["merge", "-q", "--no-ff", "-m", "m3", "feature"]);
    let m3 = t.git(&["rev-parse", "HEAD"]).trim().to_owned();
    (t, m1, f2, m2, m3)
}

#[test]
fn list_branches_reads_tips_descriptions_and_flags() {
    let t = TestRepo::new();
    t.git(&["checkout", "-q", "-b", "main"]);
    t.write("a.txt", "1");
    t.commit_all("one");
    t.write("a.txt", "2");
    t.commit_all("two");
    t.git(&["branch", "savestates/a", "HEAD~1"]);
    t.git(&["branch", "develop"]);
    t.git(&[
        "config",
        "branch.savestates/a.description",
        "Versão com menu lateral",
    ]);
    t.git(&["config", "branch.develop.savestates-archived", "true"]);

    let list = list_branches(&t.repo()).unwrap();
    let names: Vec<_> = list.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["develop", "main", "savestates/a"]);
    for b in &list {
        assert_eq!(b.tip, t.git(&["rev-parse", &b.name]).trim());
    }
    let a = list.iter().find(|b| b.name == "savestates/a").unwrap();
    assert_eq!(a.description.as_deref(), Some("Versão com menu lateral"));
    let archived: Vec<_> = list
        .iter()
        .filter(|b| b.archived)
        .map(|b| &b.name)
        .collect();
    assert_eq!(archived, ["develop"]);
    let with_worktree: Vec<_> = list
        .iter()
        .filter(|b| b.worktree.is_some())
        .map(|b| &b.name)
        .collect();
    assert_eq!(with_worktree, ["main"]);
}

#[test]
fn list_branches_marks_other_worktrees() {
    let t = TestRepo::new();
    t.git(&["checkout", "-q", "-b", "main"]);
    t.write("a.txt", "1");
    t.commit_all("one");
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("wt");
    t.git(&["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "side"]);

    let list = list_branches(&t.repo()).unwrap();
    let side = list.iter().find(|b| b.name == "side").unwrap();
    assert_eq!(
        dunce::canonicalize(side.worktree.as_ref().unwrap()).unwrap(),
        dunce::canonicalize(&wt).unwrap()
    );
}

#[test]
fn switch_new_and_switch_to_move_the_project() {
    let t = TestRepo::new();
    t.git(&["checkout", "-q", "-b", "main"]);
    t.write("a.txt", "1");
    let first = t.commit_all("one");
    t.write("a.txt", "2");
    t.commit_all("two");

    let repo = t.repo();
    switch_new(&repo, "savestates/x", &first).unwrap();
    assert_eq!(
        t.git(&["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/savestates/x"
    );
    assert_eq!(t.git(&["rev-parse", "HEAD"]).trim(), first);
    assert_eq!(
        std::fs::read_to_string(t.path().join("a.txt")).unwrap(),
        "1"
    );

    switch_to(&repo, "main").unwrap();
    assert_eq!(t.git(&["symbolic-ref", "HEAD"]).trim(), "refs/heads/main");
    assert_eq!(
        std::fs::read_to_string(t.path().join("a.txt")).unwrap(),
        "2"
    );

    create_branch(&repo, "other", &first).unwrap();
    assert_eq!(t.git(&["rev-parse", "other"]).trim(), first);
}

#[test]
fn delete_branch_removes_its_settings() {
    let t = TestRepo::new();
    t.git(&["checkout", "-q", "-b", "main"]);
    t.write("a.txt", "1");
    t.commit_all("one");
    t.git(&["branch", "savestates/a"]);
    t.git(&["config", "branch.savestates/a.description", "x"]);
    let repo = t.repo();
    assert!(
        config_get(&repo, "branch.savestates/a.description")
            .unwrap()
            .is_some()
    );

    delete_branch(&repo, "savestates/a").unwrap();
    assert_eq!(
        config_get(&repo, "branch.savestates/a.description").unwrap(),
        None
    );
}

#[test]
fn first_parent_line_ignores_merged_in_saves() {
    let (t, m1, f2, m2, m3) = merge_repo();
    let repo = t.repo();
    assert!(!on_first_parent_line(&repo, &f2, "main").unwrap());
    assert!(on_first_parent_line(&repo, &m2, "main").unwrap());
    assert!(on_first_parent_line(&repo, &m3, "main").unwrap());
    assert!(on_first_parent_line(&repo, &m1, "feature").unwrap());
}

#[test]
fn branches_containing_lists_the_branches_that_hold_a_save() {
    let (t, m1, f2, m2, _m3) = merge_repo();
    let repo = t.repo();
    t.git(&["branch", "side", &m2]);
    assert_eq!(
        branches_containing(&repo, &m1).unwrap(),
        ["feature", "main", "side"]
    );
    assert_eq!(
        branches_containing(&repo, &f2).unwrap(),
        ["feature", "main"]
    );
    assert_eq!(branches_containing(&repo, &m2).unwrap(), ["main", "side"]);
}

#[test]
fn own_first_parent_saves_stop_at_other_tips() {
    let (t, _m1, f2, m2, m3) = merge_repo();
    let repo = t.repo();
    assert!(
        own_first_parent_saves(&repo, &f2, std::slice::from_ref(&m3))
            .unwrap()
            .is_empty()
    );
    assert_eq!(own_first_parent_saves(&repo, &m3, &[f2]).unwrap(), [m3, m2]);
}

#[test]
fn head_commit_and_branch_without_a_work_tree() {
    // Before the first commit: on a branch with no commit.
    let t = TestRepo::new();
    let repo = t.repo();
    assert_eq!(head_commit(&repo).unwrap(), None);
    assert_eq!(head_branch(&repo).unwrap().as_deref(), Some("main"));
    t.write("a.txt", "1");
    let first = t.commit_all("first");
    assert_eq!(head_commit(&repo).unwrap(), Some(first.clone()));
    // Detached: a commit and no branch.
    t.git(&["checkout", "-q", "--detach"]);
    assert_eq!(head_branch(&repo).unwrap(), None);
    assert_eq!(head_commit(&repo).unwrap(), Some(first.clone()));

    // A bare clone answers both without a work tree.
    let bare = TestRepo::empty_dir();
    let source = t.path().to_string_lossy().into_owned();
    bare.git(&["clone", "-q", "--bare", &source, "."]);
    let repo = bare.repo();
    assert_eq!(head_commit(&repo).unwrap(), Some(first));
    assert!(head_branch(&repo).unwrap().is_some());
}
