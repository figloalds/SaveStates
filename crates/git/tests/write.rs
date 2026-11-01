use savestates_git::{
    GitError, add_all, commit, config_get, diff_cached, git_path, hooks_configured,
    path_in_history, rm_cached,
};
use testkit::TestRepo;

#[test]
fn path_in_history_is_false_without_a_head() {
    let repo = TestRepo::new();
    repo.write(".env", "A=1\n");

    assert!(!path_in_history(&repo.repo(), ".env").unwrap());
}

#[test]
fn path_in_history_follows_the_files_past() {
    let repo = TestRepo::new();
    repo.write(".env", "A=1\n");
    repo.write("src/a.js", "a\n");
    repo.commit_all("one");
    repo.remove(".env");
    repo.commit_all("two");

    let git = repo.repo();
    assert!(
        path_in_history(&git, ".env").unwrap(),
        "deleted files stay in older saves"
    );
    assert!(
        path_in_history(&git, "src").unwrap(),
        "a folder counts through its files"
    );
    assert!(!path_in_history(&git, "other.txt").unwrap());
}

#[test]
fn path_in_history_takes_the_path_literally() {
    let repo = TestRepo::new();
    repo.write("pages/[id].js", "a\n");
    repo.write("pages/i.js", "i\n");
    repo.commit_all("one");

    let git = repo.repo();
    assert!(path_in_history(&git, "pages/[id].js").unwrap());
    assert!(
        !path_in_history(&git, "pages/[i].js").unwrap(),
        "[i] must not act as a glob that matches pages/i.js"
    );
}

#[test]
fn rm_cached_keeps_the_file_on_disk() {
    let repo = TestRepo::new();
    repo.write(".env", "A=1\n");
    repo.commit_all("one");

    rm_cached(&repo.repo(), ".env", false).unwrap();

    assert_eq!(repo.git(&["ls-files", ".env"]), "");
    assert!(repo.path().join(".env").is_file());
}

#[test]
fn rm_cached_recursive_takes_a_whole_folder_out_of_the_index() {
    let repo = TestRepo::new();
    repo.write("web/node_modules/a/x.js", "x\n");
    repo.write("web/node_modules/b.js", "b\n");
    repo.write("web/app.js", "app\n");
    repo.commit_all("one");

    rm_cached(&repo.repo(), "web/node_modules", true).unwrap();

    assert_eq!(repo.git(&["ls-files"]), "web/app.js\n");
    assert!(repo.path().join("web/node_modules/a/x.js").is_file());
}

#[test]
fn rm_cached_on_an_untracked_path_does_nothing() {
    let repo = TestRepo::new();
    repo.write("kept.txt", "k\n");
    repo.commit_all("one");
    repo.write("new.txt", "n\n");

    let git = repo.repo();
    rm_cached(&git, "new.txt", false).unwrap();
    rm_cached(&git, "no-such-folder", true).unwrap();

    assert_eq!(repo.git(&["ls-files"]), "kept.txt\n");
    assert!(repo.path().join("new.txt").is_file());
}

#[test]
fn rm_cached_takes_the_path_literally() {
    let repo = TestRepo::new();
    repo.write("pages/[id].js", "a\n");
    repo.write("pages/i.js", "i\n");
    repo.commit_all("one");

    rm_cached(&repo.repo(), "pages/[id].js", false).unwrap();

    assert_eq!(repo.git(&["ls-files"]), "pages/i.js\n");
}

#[test]
fn rm_cached_works_when_the_index_differs_from_both_the_file_and_head() {
    let repo = TestRepo::new();
    repo.write(".env", "PORT=1\n");
    repo.commit_all("one");
    repo.write(".env", "PORT=2\n");
    repo.git(&["add", ".env"]);
    repo.write(".env", "PORT=3\n");

    rm_cached(&repo.repo(), ".env", false).unwrap();

    assert_eq!(repo.git(&["ls-files", "--cached", ".env"]), "");
    assert_eq!(
        std::fs::read_to_string(repo.path().join(".env")).unwrap(),
        "PORT=3\n"
    );
}

#[test]
fn add_all_with_a_scratch_index_leaves_the_real_one_alone() {
    let repo = TestRepo::new();
    repo.write(
        "a.txt", "a
",
    );
    let scratch = TestRepo::empty_dir();
    let index = scratch.path().join("index");

    add_all(&repo.repo(), Some(&index)).unwrap();

    assert!(index.is_file());
    assert!(!repo.path().join(".git/index").exists());
    assert_eq!(
        repo.git(&["status", "--porcelain=v1"]),
        "?? a.txt
"
    );
    let staged = diff_cached(&repo.repo(), Some(&index), &["--name-only"]).unwrap();
    assert_eq!(
        staged,
        "a.txt
"
    );
    assert_eq!(
        diff_cached(&repo.repo(), None, &["--name-only"]).unwrap(),
        ""
    );
}

#[test]
fn commit_keeps_the_message_verbatim() {
    let repo = TestRepo::new();
    repo.write(
        "a.txt", "a
",
    );
    add_all(&repo.repo(), None).unwrap();

    commit(&repo.repo(), "# not a comment").unwrap();

    assert_eq!(
        repo.git(&["log", "-1", "--format=%B"]).trim(),
        "# not a comment"
    );
}

#[test]
fn commit_runs_the_hooks() {
    let repo = TestRepo::new();
    repo.write(
        "a.txt", "a
",
    );
    repo.write(
        ".git/hooks/pre-commit",
        "#!/bin/sh
echo nope
exit 1
",
    );
    add_all(&repo.repo(), None).unwrap();

    match commit(&repo.repo(), "one") {
        Err(GitError::Failed { stderr, .. }) => assert!(stderr.contains("nope"), "{stderr}"),
        other => panic!("expected the hook to refuse, got {other:?}"),
    }
}

#[test]
fn config_get_tells_unset_from_empty() {
    let repo = TestRepo::new();
    let git = repo.repo();

    assert_eq!(
        config_get(&git, "user.name").unwrap().as_deref(),
        Some("Test")
    );
    assert_eq!(config_get(&git, "save.states.nothing").unwrap(), None);
    repo.git(&["config", "save.states.empty", ""]);
    assert_eq!(
        config_get(&git, "save.states.empty").unwrap().as_deref(),
        Some("")
    );
}

#[test]
fn hooks_configured_ignores_samples_and_sees_hooks_path() {
    let repo = TestRepo::new();
    let git = repo.repo();
    repo.write(
        ".git/hooks/commit-msg.sample",
        "#!/bin/sh
",
    );
    assert!(!hooks_configured(&git).unwrap());

    repo.write(
        ".git/hooks/commit-msg",
        "#!/bin/sh
",
    );
    assert!(hooks_configured(&git).unwrap());
    repo.remove(".git/hooks/commit-msg");

    repo.git(&["config", "core.hooksPath", ".githooks"]);
    assert!(hooks_configured(&git).unwrap());
}

#[test]
fn git_path_is_absolute_and_inside_the_repository() {
    let repo = TestRepo::new();

    let index = git_path(&repo.repo(), "index").unwrap();

    assert!(index.is_absolute(), "{index:?}");
    assert!(index.ends_with(".git/index"), "{index:?}");
}
