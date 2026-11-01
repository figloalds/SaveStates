use savestates_git::RunOpts;
use savestates_git::{
    EntryKind, GitError, StatusEntry, blob, blob_size, first_parent, log_first_parent,
    ls_tree_names, name_status,
};
use testkit::TestRepo;

fn entry(path: &str, kind: EntryKind, orig_path: Option<&str>) -> StatusEntry {
    StatusEntry {
        path: path.to_owned(),
        kind,
        orig_path: orig_path.map(str::to_owned),
    }
}

#[test]
fn log_records_are_newest_first_with_all_fields() {
    let repo = TestRepo::new();
    repo.write("a.txt", "1");
    let first = repo.commit_all("first");
    repo.write("a.txt", "2");
    repo.write("dir/b c.txt", "2");
    let second = repo.commit_all("second: \"quoted\" ✨");

    let entries = log_first_parent(&repo.repo(), "HEAD", 0, 10).unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].oid, second);
    assert_eq!(entries[0].subject, "second: \"quoted\" ✨");
    assert_eq!(entries[0].files, 2);
    assert_eq!(entries[1].oid, first);
    assert_eq!(entries[1].files, 1);
    // `%aI` is strict ISO 8601: date, `T`, time, then an offset.
    assert!(
        entries[0].author_time.contains('T'),
        "{}",
        entries[0].author_time
    );
}

#[test]
fn log_keeps_commits_whose_author_date_git_cannot_print() {
    let repo = TestRepo::new();
    repo.write("a.txt", "1");
    repo.commit_all("fine");
    let tree = repo.git(&["rev-parse", "HEAD^{tree}"]).trim().to_owned();
    let forge = |when: &str, msg: &str| {
        let head = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
        let body = format!(
            "tree {tree}
parent {head}
author T <t@e> {when}
committer T <t@e> 1700000000 +0000

{msg}
"
        );
        let opts = RunOpts {
            stdin: Some(body.as_bytes()),
            ..RunOpts::default()
        };
        let args = [
            "hash-object",
            "-t",
            "commit",
            "-w",
            "--literally",
            "--stdin",
        ];
        let out = repo.repo().run(&args, &opts).unwrap();
        repo.git(&[
            "update-ref",
            "refs/heads/main",
            String::from_utf8(out).unwrap().trim(),
        ]);
    };
    forge("1700000000 +9999", "bad offset");
    forge("1700000000 xyz", "bad zone");
    forge("99999999999999999999 +0000", "bad timestamp");

    let entries = log_first_parent(&repo.repo(), "HEAD", 0, 10).unwrap();

    let summary: Vec<_> = entries
        .iter()
        .map(|e| (e.subject.as_str(), e.author_unix))
        .collect();
    assert_eq!(
        summary,
        [
            ("bad timestamp", None),
            ("bad zone", None),
            ("bad offset", Some(1_700_000_000)),
            ("fine", summary[3].1),
        ]
    );
    assert!(summary[3].1.is_some());
    // The offset git printed is kept as text for the caller to judge.
    assert!(
        entries[2].author_time.ends_with("+99:99"),
        "{}",
        entries[2].author_time
    );
}

#[test]
fn log_skip_and_limit() {
    let repo = TestRepo::new();
    for n in 1..=5 {
        repo.write("f.txt", n.to_string());
        repo.commit_all(&format!("s{n}"));
    }
    let subjects = |skip, limit| {
        log_first_parent(&repo.repo(), "HEAD", skip, limit)
            .unwrap()
            .into_iter()
            .map(|e| e.subject)
            .collect::<Vec<_>>()
    };

    assert_eq!(subjects(1, 2), ["s4", "s3"]);
    assert_eq!(subjects(4, 10), ["s1"]);
    assert_eq!(subjects(5, 10), Vec::<String>::new());
    assert_eq!(subjects(0, 0), Vec::<String>::new());
}

#[test]
fn log_of_a_commit_that_changes_nothing_has_no_files() {
    let repo = TestRepo::new();
    repo.write("a.txt", "1");
    repo.commit_all("first");
    repo.git(&["commit", "-q", "--allow-empty", "-m", "empty"]);
    repo.write("b.txt", "1");
    repo.commit_all("third");

    let entries = log_first_parent(&repo.repo(), "HEAD", 0, 10).unwrap();

    assert_eq!(
        entries.iter().map(|e| e.files).collect::<Vec<_>>(),
        [1, 0, 1]
    );
    assert_eq!(entries[1].subject, "empty");
}

#[test]
fn log_on_an_unborn_branch_is_empty_but_a_bad_rev_is_an_error() {
    let repo = TestRepo::new();

    let unborn = log_first_parent(&repo.repo(), "HEAD", 0, 10).unwrap();
    // A file called `HEAD` must not change that.
    repo.write("HEAD", "x");
    let unborn_with_clashing_file = log_first_parent(&repo.repo(), "HEAD", 0, 10).unwrap();
    repo.remove("HEAD");
    repo.write("a.txt", "1");
    repo.commit_all("first");
    let bad_rev = log_first_parent(&repo.repo(), "no-such-branch", 0, 10);

    assert!(unborn.is_empty());
    assert!(unborn_with_clashing_file.is_empty());
    assert!(
        matches!(bad_rev, Err(GitError::Failed { .. })),
        "{bad_rev:?}"
    );
}

#[test]
fn log_survives_a_file_named_like_the_revision() {
    let repo = TestRepo::new();
    repo.write("HEAD", "a file called HEAD");
    let oid = repo.commit_all("first");

    let entries = log_first_parent(&repo.repo(), "HEAD", 0, 10).unwrap();
    let changes = name_status(&repo.repo(), "HEAD").unwrap();
    let parent = first_parent(&repo.repo(), "HEAD").unwrap();

    assert_eq!(entries[0].oid, oid);
    assert_eq!(changes, [entry("HEAD", EntryKind::Added, None)]);
    assert_eq!(parent, None);
}

#[test]
fn name_status_reports_every_kind_with_unquoted_names() {
    let repo = TestRepo::new();
    repo.write("keep me.txt", "one\n");
    repo.write("gone.txt", "bye\n");
    repo.write(
        "moved/old name.md",
        "a long enough line\nand another one\nand a third\n",
    );
    let root = repo.commit_all("root");
    repo.write("keep me.txt", "two\n");
    repo.remove("gone.txt");
    repo.git(&["mv", "moved/old name.md", "moved/ação nova.md"]);
    repo.write("new.txt", "fresh\n");
    let second = repo.commit_all("second");

    let in_root = name_status(&repo.repo(), &root).unwrap();
    let in_second = name_status(&repo.repo(), &second).unwrap();

    assert_eq!(
        in_root,
        [
            entry("gone.txt", EntryKind::Added, None),
            entry("keep me.txt", EntryKind::Added, None),
            entry("moved/old name.md", EntryKind::Added, None),
        ]
    );
    assert_eq!(
        in_second,
        [
            entry("gone.txt", EntryKind::Deleted, None),
            entry("keep me.txt", EntryKind::Modified, None),
            entry(
                "moved/ação nova.md",
                EntryKind::Renamed,
                Some("moved/old name.md")
            ),
            entry("new.txt", EntryKind::Added, None),
        ]
    );
}

#[test]
fn name_status_of_a_merge_is_against_the_first_parent() {
    let repo = TestRepo::new();
    repo.write("base.txt", "base");
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("f1.txt", "a");
    repo.commit_all("f1");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("base.txt", "base, then main");
    repo.commit_all("main2");
    repo.git(&["merge", "-q", "--no-ff", "-m", "m", "feature"]);

    let changes = name_status(&repo.repo(), "HEAD").unwrap();

    assert_eq!(changes, [entry("f1.txt", EntryKind::Added, None)]);
}

#[test]
fn ls_tree_names_lists_nested_files_unquoted() {
    let repo = TestRepo::new();
    repo.write("a/b/c.md", "c");
    repo.write("pasta/ação nova.txt", "x");
    repo.write("top.txt", "t");
    let oid = repo.commit_all("first");

    let names = ls_tree_names(&repo.repo(), &oid).unwrap();

    assert_eq!(names, ["a/b/c.md", "pasta/ação nova.txt", "top.txt"]);
}

#[test]
fn blob_and_blob_size_read_a_path_at_a_revision() {
    let repo = TestRepo::new();
    repo.write("dir/some file.bin", b"a\0b\r\nc");
    let oid = repo.commit_all("first");
    let path = "dir/some file.bin";

    let bytes = blob(&repo.repo(), &oid, path).unwrap();
    let size = blob_size(&repo.repo(), &oid, path).unwrap();

    assert_eq!(bytes.as_deref(), Some(&b"a\0b\r\nc"[..]));
    assert_eq!(size, Some(6));
}

#[test]
fn blob_is_none_for_a_path_that_is_not_in_the_tree() {
    let repo = TestRepo::new();
    repo.write("dir/a.txt", "a");
    let oid = repo.commit_all("first");
    let repo = repo.repo();

    assert_eq!(blob(&repo, &oid, "nope.txt").unwrap(), None);
    assert_eq!(blob(&repo, &oid, "dir/nope.txt").unwrap(), None);
    assert_eq!(blob_size(&repo, &oid, "nope.txt").unwrap(), None);
}

#[test]
fn blob_size_is_none_for_a_directory() {
    let repo = TestRepo::new();
    repo.write("dir/a.txt", "a");
    let oid = repo.commit_all("first");

    let size = blob_size(&repo.repo(), &oid, "dir").unwrap();

    assert_eq!(size, None);
}

#[test]
fn first_parent_is_none_for_a_root_and_the_mainline_for_a_merge() {
    let repo = TestRepo::new();
    repo.write("base.txt", "base");
    let base = repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("f1.txt", "a");
    let f1 = repo.commit_all("f1");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("base.txt", "base, then main");
    let main2 = repo.commit_all("main2");
    repo.git(&["merge", "-q", "--no-ff", "-m", "m", "feature"]);
    let merge = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
    let repo = repo.repo();

    assert_eq!(first_parent(&repo, &base).unwrap(), None);
    assert_eq!(first_parent(&repo, &main2).unwrap(), Some(base));
    assert_eq!(first_parent(&repo, &merge).unwrap(), Some(main2));
    assert_ne!(first_parent(&repo, &f1).unwrap(), None);
}
