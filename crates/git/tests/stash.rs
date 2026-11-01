use std::fs;

use savestates_git::{
    GitError, refs_with_prefix, snapshot_files_to_ref, stash_apply, stash_snapshot_paths_to_ref,
    stash_snapshot_to_ref, status,
};
use testkit::TestRepo;

const PREFIX: &str = "refs/savestates/recovery/";

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// One save, with `.env` and `build/` ignored and present on disk.
fn repo_with_ignored_files() -> TestRepo {
    let repo = TestRepo::new();
    repo.write(".gitignore", ".env\nbuild/\n");
    repo.write("a.txt", "v1");
    repo.commit_all("s1");
    repo.write(".env", "secret");
    repo.write("build/out.js", "compiled");
    repo
}

#[test]
fn snapshot_of_files_has_the_shape_of_a_stash_and_touches_nothing() {
    let repo = repo_with_ignored_files();
    let ref_name = format!("{PREFIX}1-0");

    let oid = snapshot_files_to_ref(&repo.repo(), &names(&[".env", "build/out.js"]), &ref_name)
        .unwrap()
        .expect("a snapshot");

    // HEAD, an index commit, and a commit holding the files, as `git stash` makes them.
    let parents = repo.git(&["rev-list", "--parents", "-n", "1", &oid]);
    assert_eq!(parents.split_whitespace().count(), 4, "{parents}");
    assert_eq!(
        repo.git(&["ls-tree", "-r", "--name-only", &format!("{oid}^3")]),
        ".env\nbuild/out.js\n"
    );
    assert_eq!(repo.git(&["show", &format!("{oid}^3:.env")]), "secret");
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an <%ae>", &oid]).trim(),
        "SaveStates <savestates@localhost>"
    );
    assert_eq!(
        refs_with_prefix(&repo.repo(), PREFIX).unwrap(),
        vec![(ref_name, oid)]
    );
    // Nothing else moved: files, index and stash list.
    assert_eq!(
        fs::read_to_string(repo.path().join(".env")).unwrap(),
        "secret"
    );
    assert!(repo.path().join("build/out.js").is_file());
    assert_eq!(repo.git(&["stash", "list"]), "");
    assert!(status(&repo.repo()).unwrap().entries.is_empty());
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "");
}

#[test]
fn a_snapshot_of_files_applies_like_a_stash() {
    let repo = repo_with_ignored_files();
    let oid = snapshot_files_to_ref(&repo.repo(), &names(&[".env"]), &format!("{PREFIX}1-0"))
        .unwrap()
        .unwrap();
    fs::remove_file(repo.path().join(".env")).unwrap();

    stash_apply(&repo.repo(), &oid).unwrap();

    assert_eq!(
        fs::read_to_string(repo.path().join(".env")).unwrap(),
        "secret"
    );
}

#[test]
fn file_names_are_taken_literally() {
    let repo = TestRepo::new();
    repo.write(".gitignore", "src/\n");
    repo.write("a.txt", "v1");
    repo.commit_all("s1");
    repo.write("src/[id].js", "bracket");
    repo.write("src/i.js", "plain");

    let oid = snapshot_files_to_ref(
        &repo.repo(),
        &names(&["src/[id].js"]),
        &format!("{PREFIX}1-0"),
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        repo.git(&["ls-tree", "-r", "--name-only", &format!("{oid}^3")]),
        "src/[id].js\n"
    );
}

#[test]
fn an_existing_ref_is_never_overwritten() {
    let repo = repo_with_ignored_files();
    let ref_name = format!("{PREFIX}1-0");
    let first = snapshot_files_to_ref(&repo.repo(), &names(&[".env"]), &ref_name)
        .unwrap()
        .unwrap();

    let second = snapshot_files_to_ref(&repo.repo(), &names(&["build/out.js"]), &ref_name);

    assert!(matches!(second, Err(GitError::Failed { .. })), "{second:?}");
    assert_eq!(
        refs_with_prefix(&repo.repo(), PREFIX).unwrap(),
        vec![(ref_name, first)]
    );
}

#[test]
fn nothing_to_snapshot_and_nothing_to_base_it_on() {
    let repo = repo_with_ignored_files();
    assert!(
        snapshot_files_to_ref(&repo.repo(), &[], &format!("{PREFIX}1-0"))
            .unwrap()
            .is_none()
    );
    assert!(
        snapshot_files_to_ref(
            &repo.repo(),
            &names(&["missing.txt"]),
            &format!("{PREFIX}1-0")
        )
        .is_err(),
        "a file that is not there"
    );
    assert!(refs_with_prefix(&repo.repo(), PREFIX).unwrap().is_empty());

    let empty = TestRepo::new();
    empty.write("a.txt", "v1");
    assert!(
        snapshot_files_to_ref(&empty.repo(), &names(&["a.txt"]), &format!("{PREFIX}1-0")).is_err()
    );
}

#[test]
fn stash_snapshot_of_nothing_is_none_and_leaves_the_users_stash() {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    repo.commit_all("s1");
    repo.write("a.txt", "mine");
    repo.git(&["stash", "push", "-m", "mine"]);

    let snapshot = stash_snapshot_to_ref(&repo.repo(), None, &format!("{PREFIX}1-0")).unwrap();

    assert!(snapshot.is_none());
    assert!(repo.git(&["stash", "list"]).contains("mine"));
    assert!(refs_with_prefix(&repo.repo(), PREFIX).unwrap().is_empty());
}

#[test]
fn stash_snapshot_of_several_paths_is_one_entry_and_leaves_the_rest() {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    repo.write("c.txt", "v1");
    repo.write("src/[id].js", "v1");
    repo.write("src/i.js", "v1");
    repo.commit_all("s1");
    repo.write("a.txt", "changed");
    repo.write("c.txt", "changed");
    repo.write("src/[id].js", "changed");
    repo.write("src/i.js", "changed");
    repo.write("new/x.txt", "new");
    repo.write("new/y.txt", "new");
    repo.write("other.txt", "new");
    let ref_name = format!("{PREFIX}1-0");

    let oid = stash_snapshot_paths_to_ref(
        &repo.repo(),
        &names(&["a.txt", "src/[id].js", "new"]),
        &ref_name,
    )
    .unwrap()
    .expect("a snapshot");

    assert_eq!(
        repo.git(&["diff", "--name-only", &format!("{oid}^1"), &oid]),
        "a.txt\nsrc/[id].js\n"
    );
    assert_eq!(
        repo.git(&["ls-tree", "-r", "--name-only", &format!("{oid}^3")]),
        "new/x.txt\nnew/y.txt\n"
    );
    // What was not named is still there, changed.
    let left = status(&repo.repo()).unwrap();
    let mut left: Vec<_> = left.entries.into_iter().map(|e| e.path).collect();
    left.sort();
    assert_eq!(left, names(&["c.txt", "other.txt", "src/i.js"]));
    assert_eq!(repo.git(&["stash", "list"]), "");
    assert_eq!(
        refs_with_prefix(&repo.repo(), PREFIX).unwrap(),
        vec![(ref_name, oid)]
    );
}

#[test]
fn stash_snapshot_of_no_paths_takes_nothing() {
    let repo = TestRepo::new();
    repo.write("a.txt", "v1");
    repo.commit_all("s1");
    repo.write("a.txt", "changed");
    repo.write("b.txt", "new");

    let snapshot = stash_snapshot_paths_to_ref(&repo.repo(), &[], &format!("{PREFIX}1-0")).unwrap();

    assert!(snapshot.is_none());
    assert_eq!(status(&repo.repo()).unwrap().entries.len(), 2);
    assert!(refs_with_prefix(&repo.repo(), PREFIX).unwrap().is_empty());
}
