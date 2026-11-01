use savestates_core::{
    Change, ChangeKind, CoreError, FileContent, HISTORY_PAGE, MAX_DIFF_BYTES, OpenOutcome, Project,
    SaveId, SaveState, Side, changes_in_save, classify, files_at_save, open_project, read_file,
    timeline_page,
};
use savestates_git::RunOpts;
use testkit::TestRepo;

fn open(repo: &TestRepo) -> Project {
    match open_project(&repo.exe(), repo.path()).unwrap() {
        OpenOutcome::Opened(project) => project,
        other => panic!("expected Opened, got {other:?}"),
    }
}

fn names(saves: &[SaveState]) -> Vec<&str> {
    saves.iter().map(|s| s.name.as_str()).collect()
}

fn change(path: &str, kind: ChangeKind) -> Change {
    Change {
        path: path.to_owned(),
        kind,
    }
}

/// `main` has `base` and `main2`; `feature` branched from `base` with `f1` and was merged
/// into `main` with a merge commit named `m`. Left on `main`.
fn merged_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.write("base.txt", "base");
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("f1a.txt", "a");
    repo.write("f1b.txt", "b");
    repo.commit_all("f1");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("base.txt", "base, then main");
    repo.commit_all("main2");
    repo.git(&["merge", "-q", "--no-ff", "-m", "m", "feature"]);
    repo
}

/// Adds a commit on top of `HEAD` (same files) whose author line ends in `when`, written
/// straight into the object database so git does not get to reject or fix the date.
fn forge_commit(repo: &TestRepo, when: &str, msg: &str, literally: bool) {
    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
    let tree = repo.git(&["rev-parse", "HEAD^{tree}"]).trim().to_owned();
    let body = format!(
        "tree {tree}
parent {head}
author T <t@e> {when}
committer T <t@e> 1700000000 +0000

{msg}
"
    );
    let mut args = vec!["hash-object", "-t", "commit", "-w", "--stdin"];
    if literally {
        args.push("--literally");
    }
    let opts = RunOpts {
        stdin: Some(body.as_bytes()),
        ..RunOpts::default()
    };
    let out = repo.repo().run(&args, &opts).unwrap();
    let oid = String::from_utf8(out).unwrap();
    repo.git(&["update-ref", "refs/heads/main", oid.trim()]);
}

#[test]
fn newest_first_with_counts() {
    let repo = TestRepo::new();
    repo.write("a.txt", "1");
    let first = repo.commit_all("first");
    repo.write("a.txt", "2");
    repo.write("b.txt", "2");
    let second = repo.commit_all("second");
    repo.write("b.txt", "3");
    let third = repo.commit_all("third");

    let saves = timeline_page(&open(&repo), 0, HISTORY_PAGE).unwrap();

    assert_eq!(names(&saves), ["third", "second", "first"]);
    assert_eq!(
        saves.iter().map(|s| s.files_changed).collect::<Vec<_>>(),
        [1, 2, 1]
    );
    assert_eq!(
        saves.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
        [SaveId(third), SaveId(second), SaveId(first)]
    );
}

#[test]
fn save_time_keeps_the_author_offset() {
    let repo = TestRepo::new();
    repo.write("a.txt", "1");
    repo.git(&["add", "-A"]);
    repo.git(&[
        "commit",
        "-q",
        "--cleanup=verbatim",
        "--date=2020-01-02T03:04:05-03:00",
        "-m",
        "dated",
    ]);

    let saves = timeline_page(&open(&repo), 0, HISTORY_PAGE).unwrap();

    assert_eq!(saves[0].time.to_rfc3339(), "2020-01-02T03:04:05-03:00");
}

#[test]
fn a_commit_with_a_malformed_author_date_still_shows_up() {
    let repo = TestRepo::new();
    repo.write("a.txt", "1");
    repo.git(&["add", "-A"]);
    repo.git(&[
        "commit",
        "-q",
        "--cleanup=verbatim",
        "--date=2020-01-02T03:04:05-03:00",
        "-m",
        "fine",
    ]);
    // Git prints these time zones as `+99:99`, `+24:00` and `+05:60`, which are not RFC 3339.
    forge_commit(&repo, "1700000000 +9999", "bad offset", false);
    forge_commit(&repo, "1700000000 +2400", "bad hours", false);
    forge_commit(&repo, "1700000000 +0560", "bad minutes", false);
    // Git prints no date at all for a non-numeric time zone; only `--literally` can write one.
    forge_commit(&repo, "1700000000 xyz", "bad zone", true);

    let saves = timeline_page(&open(&repo), 0, HISTORY_PAGE).unwrap();

    assert_eq!(
        names(&saves),
        ["bad zone", "bad minutes", "bad hours", "bad offset", "fine"]
    );
    // No usable date: the Unix epoch, at UTC.
    assert_eq!(saves[0].time.to_rfc3339(), "1970-01-01T00:00:00+00:00");
    // Bad offset: the commit's own Unix timestamp, at UTC.
    for save in &saves[1..4] {
        assert_eq!(save.time.to_rfc3339(), "2023-11-14T22:13:20+00:00");
    }
    // The well-formed save is unaffected.
    assert_eq!(saves[4].time.to_rfc3339(), "2020-01-02T03:04:05-03:00");
}

#[test]
fn paging() {
    let repo = TestRepo::new();
    for n in 1..=7 {
        repo.write("f.txt", n.to_string());
        repo.commit_all(&format!("s{n}"));
    }
    let project = open(&repo);

    let middle = timeline_page(&project, 2, 3).unwrap();
    let tail = timeline_page(&project, 5, 10).unwrap();
    let past_the_end = timeline_page(&project, 7, 10).unwrap();

    assert_eq!(names(&middle), ["s5", "s4", "s3"]);
    assert_eq!(names(&tail), ["s2", "s1"]);
    assert!(past_the_end.is_empty());
}

#[test]
fn first_parent_only() {
    let repo = merged_repo();

    let saves = timeline_page(&open(&repo), 0, HISTORY_PAGE).unwrap();

    assert_eq!(names(&saves), ["m", "main2", "base"]);
    // `f1` touched two files, so the merge brought two files into `main`.
    assert_eq!(saves[0].files_changed, 2);
}

#[test]
fn empty_repo_timeline_is_empty() {
    let repo = TestRepo::new();

    let saves = timeline_page(&open(&repo), 0, HISTORY_PAGE);

    assert_eq!(saves.unwrap(), vec![]);
}

#[test]
fn special_character_names_round_trip() {
    let name = "#1 \"menu\" ✨ ação";
    let repo = TestRepo::new();
    repo.write("a.txt", "1");
    repo.commit_all(name);

    let saves = timeline_page(&open(&repo), 0, HISTORY_PAGE).unwrap();

    assert_eq!(saves[0].name, name);
}

#[test]
fn changes_in_root_and_rename() {
    let repo = TestRepo::new();
    repo.write(
        "a.txt",
        "the quick brown fox\njumps over the lazy dog\nagain and again\n",
    );
    repo.write("dir/b.txt", "bee\n");
    let root = SaveId(repo.commit_all("root"));
    repo.git(&["mv", "a.txt", "renamed.txt"]);
    let renamed = SaveId(repo.commit_all("rename"));
    let project = open(&repo);

    let in_root = changes_in_save(&project, &root).unwrap();
    let in_rename = changes_in_save(&project, &renamed).unwrap();

    assert_eq!(
        in_root,
        [
            change("a.txt", ChangeKind::New),
            change("dir/b.txt", ChangeKind::New)
        ]
    );
    assert_eq!(
        in_rename,
        [change(
            "renamed.txt",
            ChangeKind::Renamed {
                from: "a.txt".to_owned()
            }
        )]
    );
}

#[test]
fn changes_cover_changed_and_deleted_with_odd_names() {
    let repo = TestRepo::new();
    repo.write("keep me.txt", "one\n");
    repo.write("gone.txt", "bye\n");
    repo.write("pasta/ação.md", "x\n");
    repo.commit_all("root");
    repo.write("keep me.txt", "two\n");
    repo.remove("gone.txt");
    repo.write("pasta/ação.md", "y\n");
    let second = SaveId(repo.commit_all("second"));

    let changes = changes_in_save(&open(&repo), &second).unwrap();

    assert_eq!(
        changes,
        [
            change("gone.txt", ChangeKind::Deleted),
            change("keep me.txt", ChangeKind::Changed),
            change("pasta/ação.md", ChangeKind::Changed),
        ]
    );
}

#[test]
fn changes_in_a_merge_are_against_the_first_parent() {
    let repo = merged_repo();
    let merge = SaveId(repo.git(&["rev-parse", "HEAD"]).trim().to_owned());

    let changes = changes_in_save(&open(&repo), &merge).unwrap();

    assert_eq!(
        changes,
        [
            change("f1a.txt", ChangeKind::New),
            change("f1b.txt", ChangeKind::New)
        ]
    );
}

#[test]
fn files_at_save_lists_tree() {
    let repo = TestRepo::new();
    repo.write("a/b/c.md", "c");
    repo.write("top.txt", "t");
    let first = SaveId(repo.commit_all("first"));
    repo.remove("top.txt");
    let second = SaveId(repo.commit_all("second"));
    let project = open(&repo);

    let at_first = files_at_save(&project, &first).unwrap();
    let at_second = files_at_save(&project, &second).unwrap();

    assert_eq!(at_first, ["a/b/c.md", "top.txt"]);
    assert_eq!(at_second, ["a/b/c.md"]);
}

#[test]
fn read_file_sides() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    let root = SaveId(repo.commit_all("root"));
    repo.write("a.txt", "two\n");
    let second = SaveId(repo.commit_all("second"));
    let project = open(&repo);
    let read = |side: Side, path: &str| read_file(&project, &side, path, false).unwrap();
    let text = |s: &str| FileContent::Text(s.to_owned());

    assert_eq!(read(Side::Save(root.clone()), "a.txt"), text("one\n"));
    assert_eq!(read(Side::Save(second.clone()), "a.txt"), text("two\n"));
    assert_eq!(read(Side::Before(second), "a.txt"), text("one\n"));
    assert_eq!(
        read(Side::Before(root.clone()), "a.txt"),
        FileContent::Missing
    );
    assert_eq!(read(Side::Save(root), "nope.txt"), FileContent::Missing);
    assert_eq!(read(Side::Now, "a.txt"), text("two\n"));

    repo.write("a.txt", "unsaved\n");
    assert_eq!(read(Side::Now, "a.txt"), text("unsaved\n"));

    repo.remove("a.txt");
    assert_eq!(read(Side::Now, "a.txt"), FileContent::Missing);
}

#[test]
fn read_file_binary_and_too_large() {
    let repo = TestRepo::new();
    repo.write("bin.dat", b"a\0b");
    let big = "x".repeat(2 * 1024 * 1024);
    repo.write("big.txt", &big);
    let save = SaveId(repo.commit_all("root"));
    let project = open(&repo);
    let read =
        |side: Side, path: &str, force: bool| read_file(&project, &side, path, force).unwrap();
    let too_large = FileContent::TooLarge { bytes: 2_097_152 };

    assert_eq!(
        read(Side::Save(save.clone()), "bin.dat", false),
        FileContent::Binary
    );
    assert_eq!(read(Side::Now, "bin.dat", false), FileContent::Binary);

    assert_eq!(read(Side::Save(save.clone()), "big.txt", false), too_large);
    assert_eq!(read(Side::Now, "big.txt", false), too_large);
    assert_eq!(
        read(Side::Save(save), "big.txt", true),
        FileContent::Text(big.clone())
    );
    assert_eq!(read(Side::Now, "big.txt", true), FileContent::Text(big));
}

#[test]
fn read_file_refuses_paths_outside_the_project() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    let save = SaveId(repo.commit_all("root"));
    let project = open(&repo);

    for path in [
        "../a.txt",
        "a/../../a.txt",
        "",
        "/etc/passwd",
        "C:/Windows/win.ini",
    ] {
        for side in [
            Side::Now,
            Side::Save(save.clone()),
            Side::Before(save.clone()),
        ] {
            let result = read_file(&project, &side, path, false);
            assert!(
                matches!(result, Err(CoreError::NotFound(_))),
                "{side:?} {path:?}: {result:?}"
            );
        }
    }
}

#[test]
fn classify_looks_for_nul_in_the_first_8000_bytes() {
    let mut nul_last_sniffed = vec![b'a'; 8_000];
    nul_last_sniffed[7_999] = 0;
    let mut nul_just_past = vec![b'a'; 8_001];
    nul_just_past[8_000] = 0;

    assert_eq!(classify(nul_last_sniffed), FileContent::Binary);
    assert_eq!(classify(b"\0".to_vec()), FileContent::Binary);
    assert!(matches!(classify(nul_just_past), FileContent::Text(_)));
    assert_eq!(classify(Vec::new()), FileContent::Text(String::new()));
    assert_eq!(
        classify("ação ✨".as_bytes().to_vec()),
        FileContent::Text("ação ✨".to_owned())
    );
    assert_eq!(
        classify(b"ok \xff ok".to_vec()),
        FileContent::Text("ok \u{fffd} ok".to_owned())
    );
}

#[test]
fn max_diff_bytes_is_one_mebibyte() {
    assert_eq!(MAX_DIFF_BYTES, 1_048_576);
}
