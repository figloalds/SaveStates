use savestates_git::{path_in_history, rm_cached};
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
