use savestates_git::{GitError, MIN_VERSION, RunOpts, Version, locate, parse_version, run_in};
use testkit::TestRepo;

#[test]
fn parses_windows_version() {
    assert_eq!(
        parse_version("git version 2.50.1.windows.1"),
        Some(Version {
            major: 2,
            minor: 50,
            patch: 1
        })
    );
    assert_eq!(
        parse_version("git version 2.31.0\n"),
        Some(Version {
            major: 2,
            minor: 31,
            patch: 0
        })
    );
    assert_eq!(parse_version("garbage"), None);
}

#[test]
fn locates_supported_git() {
    assert!(locate().unwrap().version >= MIN_VERSION);
}

#[test]
fn not_a_repo_is_classified() {
    let d = TestRepo::empty_dir();
    assert!(matches!(
        run_in(
            &d.exe(),
            d.path(),
            &["rev-parse", "--show-toplevel"],
            &RunOpts::default()
        ),
        Err(GitError::NotARepo)
    ));
}

#[test]
fn lock_is_classified() {
    let r = TestRepo::new();
    r.write("a.txt", "x");
    r.write(".git/index.lock", "");
    assert!(matches!(
        r.repo().run(&["add", "-A"], &RunOpts::default()),
        Err(GitError::Locked)
    ));
}

#[test]
fn failure_carries_stderr() {
    let r = TestRepo::new();
    match r
        .repo()
        .run(&["rev-parse", "--verify", "nope"], &RunOpts::default())
    {
        Err(GitError::Failed {
            code: Some(128),
            stderr,
            ..
        }) => assert!(!stderr.is_empty()),
        other => panic!("{other:?}"),
    }
}
