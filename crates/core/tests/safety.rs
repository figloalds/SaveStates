use std::fs;
use std::time::{Duration, Instant};

use savestates_core::gitignore::append_lines;
use savestates_core::{
    Change, ChangeKind, Decision, Flag, FlagAction, FlagReason, JUNK_DIRS, OpenOutcome, Project,
    allowed_actions, apply_decisions, find_secret_line, is_secret_name, open_project, scan,
    snapshot,
};
use testkit::TestRepo;

const LARGE_MB: u64 = 25;
const MB: usize = 1024 * 1024;

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

fn flag(path: &str, reason: FlagReason, already_saved: bool, in_older_saves: bool) -> Flag {
    Flag {
        path: path.to_owned(),
        reason,
        already_saved,
        in_older_saves,
    }
}

/// What a Quick Save would be about to save right now.
fn scan_now(repo: &TestRepo, large_file_mb: u64) -> Vec<Flag> {
    let project = open(repo);
    scan(&project, &snapshot(&project).changes, large_file_mb).unwrap()
}

fn gitignore(repo: &TestRepo) -> String {
    fs::read_to_string(repo.path().join(".gitignore")).unwrap_or_default()
}

/// A line that matches the `AKIA` pattern, assembled so no real-looking key sits in the source.
fn aws_key() -> String {
    format!("AKIA{}", "ABCDEFGH12345678")
}

#[test]
fn secret_names() {
    for name in [
        ".env",
        ".env.local",
        ".ENV",
        "server.pem",
        "private.key",
        "cert.p12",
        "cert.pfx",
        "id_rsa",
        "id_rsa.pub",
        "id_ed25519",
        "credentials.json",
        "credentials-prod.json",
        "app.keystore",
        "putty.ppk",
        "SERVER.PPK",
    ] {
        assert!(is_secret_name(name), "{name} should be a secret name");
    }
    for name in [
        ".env.example",
        ".ENV.EXAMPLE",
        "environment.ts",
        "keys.md",
        "monkey.js",
        "credentials.txt",
        "my-id_rsa",
        "ppk.txt",
    ] {
        assert!(!is_secret_name(name), "{name} should not be a secret name");
    }
}

#[test]
fn secret_content() {
    let samples = [
        "-----BEGIN RSA PRIVATE KEY-----".to_owned(),
        "-----BEGIN PRIVATE KEY-----".to_owned(),
        "-----BEGIN OPENSSH PRIVATE KEY-----".to_owned(),
        aws_key(),
        format!("ghp_{}", "a".repeat(36)),
        format!("github_pat_{}", "A1_".repeat(8)),
        format!("sk-ant-api03-{}", "x".repeat(20)),
        format!("sk-{}", "Z9".repeat(12)),
        "xoxb-1234567890-abcdef".to_owned(),
        "xoxp-1234567890-abcdef".to_owned(),
        format!("AIza{}", "B".repeat(35)),
    ];
    for sample in &samples {
        let text = format!("first line\nconst token = \"{sample}\";\n");
        assert_eq!(find_secret_line(&text), Some(2), "no match for {sample}");
    }

    assert_eq!(find_secret_line("sk-short"), None);
    assert_eq!(find_secret_line("let x = 1;"), None);
    assert_eq!(find_secret_line(""), None);
    assert_eq!(find_secret_line("AKIA1234"), None, "too short to be a key");
}

#[test]
fn secret_content_counts_lines_of_any_ending() {
    let text = format!("a\r\nb\r\n{}\r\n", aws_key());
    assert_eq!(find_secret_line(&text), Some(3));
    // The first match wins.
    let two = format!("{k}\n{k}\n", k = aws_key());
    assert_eq!(find_secret_line(&two), Some(1));
}

#[test]
fn allowed_actions_table() {
    use FlagAction::{CancelAndFix, LeaveOut, SaveAnyway, StopSaving};
    use FlagReason::{JunkFolder, Large, SecretContent, SecretName};

    let actions = |reason, already_saved| allowed_actions(&flag("x", reason, already_saved, false));

    assert_eq!(actions(SecretName, false), [LeaveOut, SaveAnyway]);
    assert_eq!(actions(JunkFolder, false), [LeaveOut, SaveAnyway]);
    assert_eq!(actions(SecretName, true), [StopSaving, SaveAnyway]);
    assert_eq!(actions(JunkFolder, true), [StopSaving, SaveAnyway]);
    assert_eq!(
        actions(SecretContent { line: 4 }, false),
        [CancelAndFix, SaveAnyway]
    );
    assert_eq!(
        actions(SecretContent { line: 4 }, true),
        [CancelAndFix, SaveAnyway]
    );
    assert_eq!(actions(Large { bytes: 9 }, false), [LeaveOut, SaveAnyway]);
    assert_eq!(actions(Large { bytes: 9 }, true), [StopSaving, SaveAnyway]);
}

#[test]
fn junk_dirs_are_the_documented_list() {
    assert_eq!(
        JUNK_DIRS,
        [
            "node_modules",
            "dist",
            "build",
            "out",
            ".next",
            "target",
            "__pycache__",
            ".venv",
            "venv"
        ]
    );
}

#[test]
fn scan_flags_a_new_secret_named_file() {
    let repo = TestRepo::new();
    repo.write(".env", "PORT=3000\n");

    assert_eq!(
        scan_now(&repo, LARGE_MB),
        [flag(".env", FlagReason::SecretName, false, false)]
    );
}

#[test]
fn scan_flags_a_modified_committed_secret_named_file_as_already_saved() {
    let repo = TestRepo::new();
    repo.write(".env", "PORT=3000\n");
    repo.commit_all("one");
    repo.write(".env", "PORT=3001\n");

    assert_eq!(
        scan_now(&repo, LARGE_MB),
        [flag(".env", FlagReason::SecretName, true, true)]
    );
}

#[test]
fn scan_remembers_a_secret_named_file_that_is_back_after_being_deleted() {
    let repo = TestRepo::new();
    repo.write(".env", "PORT=3000\n");
    repo.write("a.txt", "a\n");
    repo.commit_all("one");
    repo.remove(".env");
    repo.commit_all("two, without it");
    repo.write(".env", "PORT=3000\n");

    assert_eq!(
        scan_now(&repo, LARGE_MB),
        [flag(".env", FlagReason::SecretName, false, true)]
    );
}

#[test]
fn scan_flags_a_key_in_a_new_file_at_its_line() {
    let repo = TestRepo::new();
    let source = format!(
        "const a = 1;\nconst b = 2;\nconst key = \"{}\";\n",
        aws_key()
    );
    repo.write("src/api.js", source);

    assert_eq!(
        scan_now(&repo, LARGE_MB),
        [flag(
            "src/api.js",
            FlagReason::SecretContent { line: 3 },
            false,
            false
        )]
    );
}

#[test]
fn scan_ignores_a_key_that_the_last_save_already_had() {
    let repo = TestRepo::new();
    let key_line = format!("const key = \"{}\";\n", aws_key());
    repo.write("src/api.js", format!("{key_line}const x = 1;\n"));
    repo.commit_all("one");
    repo.write("src/api.js", format!("{key_line}const x = 2;\n"));

    assert_eq!(scan_now(&repo, LARGE_MB), []);
}

#[test]
fn scan_flags_only_a_key_added_to_a_tracked_file() {
    let repo = TestRepo::new();
    let key_line = format!("const key = \"{}\";\n", aws_key());
    repo.write("src/api.js", format!("{key_line}const x = 1;\n"));
    repo.commit_all("one");
    // The old key moves down a line, and a different key is added on line 4.
    let other = format!("const other = \"ghp_{}\";\n", "b".repeat(36));
    repo.write(
        "src/api.js",
        format!("// moved\n{key_line}const x = 1;\n{other}"),
    );

    assert_eq!(
        scan_now(&repo, LARGE_MB),
        [flag(
            "src/api.js",
            FlagReason::SecretContent { line: 4 },
            true,
            true
        )]
    );
}

#[test]
fn scan_compares_lines_not_line_endings() {
    let repo = TestRepo::new();
    let key_line = aws_key();
    repo.write("a.js", format!("{key_line}\nx\n"));
    repo.commit_all("one");
    // The same lines, now with Windows line endings.
    repo.write("a.js", format!("{key_line}\r\nx\r\n"));

    assert_eq!(scan_now(&repo, LARGE_MB), []);
}

#[test]
fn scan_checks_the_new_name_of_a_renamed_file_against_the_old_content() {
    let repo = TestRepo::new();
    let key_line = format!("const key = \"{}\";\n", aws_key());
    let body: String = (1..=12)
        .map(|i| format!("const line{i} = {i};\n"))
        .collect();
    repo.write("old.js", format!("{key_line}{body}"));
    repo.commit_all("one");
    repo.git(&["mv", "old.js", "new.js"]);

    // A pure rename carries the old key along: nothing new to warn about.
    let project = open(&repo);
    let changes = snapshot(&project).changes;
    assert_eq!(
        changes,
        [change(
            "new.js",
            ChangeKind::Renamed {
                from: "old.js".to_owned()
            }
        )]
    );
    assert_eq!(scan(&project, &changes, LARGE_MB).unwrap(), []);

    // A new key written during the rename is.
    let added = format!("const t = \"ghp_{}\";\n", "c".repeat(36));
    repo.write("new.js", format!("{key_line}{body}{added}"));
    repo.git(&["add", "new.js"]);
    let flags = scan(&project, &snapshot(&project).changes, LARGE_MB).unwrap();
    assert_eq!(
        flags,
        [flag(
            "new.js",
            FlagReason::SecretContent { line: 14 },
            true,
            false
        )]
    );
}

#[test]
fn scan_skips_binary_and_big_text_for_content() {
    let repo = TestRepo::new();
    let key = aws_key();
    let mut binary = vec![0u8];
    binary.extend_from_slice(key.as_bytes());
    repo.write("a.bin", binary);
    // Just over 1 MB of text with a key in it.
    repo.write("big.txt", format!("{key}\n{}", "x".repeat(MB)));

    assert_eq!(scan_now(&repo, LARGE_MB), []);
}

#[test]
fn scan_flags_a_large_file_by_its_size_on_disk() {
    let repo = TestRepo::new();
    repo.write("video.bin", vec![0u8; 26 * MB]);
    repo.write("small.bin", vec![0u8; MB]);

    assert_eq!(
        scan_now(&repo, LARGE_MB),
        [flag(
            "video.bin",
            FlagReason::Large {
                bytes: 26 * MB as u64
            },
            false,
            false
        )]
    );
}

#[test]
fn scan_flags_a_file_only_when_it_is_over_the_limit() {
    let repo = TestRepo::new();
    repo.write("exact.bin", vec![0u8; MB]);
    repo.write("over.bin", vec![0u8; MB + 1]);

    assert_eq!(
        scan_now(&repo, 1),
        [flag(
            "over.bin",
            FlagReason::Large {
                bytes: MB as u64 + 1
            },
            false,
            false
        )]
    );
}

#[test]
fn scan_flags_an_already_saved_large_file() {
    let repo = TestRepo::new();
    repo.write("video.bin", vec![0u8; 2 * MB]);
    repo.commit_all("one");
    repo.write("video.bin", vec![1u8; 3 * MB]);

    assert_eq!(
        scan_now(&repo, 1),
        [flag(
            "video.bin",
            FlagReason::Large {
                bytes: 3 * MB as u64
            },
            true,
            true
        )]
    );
}

#[test]
fn scan_gives_a_file_one_flag_with_the_name_first() {
    let repo = TestRepo::new();
    repo.write(".env", format!("KEY={}\n", aws_key()));

    assert_eq!(
        scan_now(&repo, LARGE_MB),
        [flag(".env", FlagReason::SecretName, false, false)]
    );
}

#[test]
fn scan_leaves_ordinary_and_example_files_alone() {
    let repo = TestRepo::new();
    repo.write(".env.example", "PORT=\n");
    repo.write("src/main.rs", "fn main() {}\n");
    repo.write("build", "#!/bin/sh\n"); // a file named like a junk folder is not a folder

    assert_eq!(scan_now(&repo, LARGE_MB), []);
}

#[test]
fn scan_never_flags_a_deleted_file() {
    let repo = TestRepo::new();
    repo.write(".env", "PORT=3000\n");
    repo.write("node_modules/a.js", "a\n");
    repo.commit_all("one");
    repo.remove(".env");
    repo.remove("node_modules");

    assert_eq!(scan_now(&repo, LARGE_MB), []);
}

#[test]
fn scan_gives_a_file_in_a_junk_folder_only_the_folder_flag() {
    let repo = TestRepo::new();
    repo.write("node_modules/pkg/.env", "A=1\n");
    repo.write(
        "node_modules/pkg/key.js",
        format!("const k = \"{}\";\n", aws_key()),
    );
    repo.write("node_modules/pkg/big.bin", vec![0u8; 2 * MB]);

    assert_eq!(
        scan_now(&repo, 1),
        [flag("node_modules", FlagReason::JunkFolder, false, false)]
    );
}

#[test]
fn scan_treats_a_tracked_junk_folder_as_already_saved() {
    let repo = TestRepo::new();
    repo.write("dist/a.js", "a\n");
    repo.commit_all("one");
    repo.write("dist/a.js", "b\n");
    repo.write("dist/new.js", "n\n");

    assert_eq!(
        scan_now(&repo, LARGE_MB),
        [flag("dist", FlagReason::JunkFolder, true, true)]
    );
}

#[test]
fn junk_grouping() {
    let repo = TestRepo::new();
    for i in 0..2000 {
        repo.write(
            &format!("node_modules/pkg{i}/index.js"),
            "module.exports = 1;\n",
        );
    }
    let project = open(&repo);
    let changes = snapshot(&project).changes;
    assert_eq!(changes.len(), 2000);

    let started = Instant::now();
    let flags = scan(&project, &changes, LARGE_MB).unwrap();
    let took = started.elapsed();

    assert_eq!(
        flags,
        [flag("node_modules", FlagReason::JunkFolder, false, false)]
    );
    assert!(
        took < Duration::from_secs(2),
        "scanning 2000 junk files took {took:?}"
    );
}

#[test]
fn junk_grouping_uses_the_shallowest_folder_and_keeps_the_path() {
    let repo = TestRepo::new();
    repo.write("web/node_modules/x.js", "x\n");
    repo.write("web/node_modules/deep/node_modules/y.js", "y\n");
    repo.write("a/dist/b/build/c.js", "c\n");
    repo.write("a/dist/other.js", "o\n");
    repo.write("target/debug/app", "bin\n");
    repo.write("src/venv/x.py", "x\n");

    let mut paths: Vec<String> = scan_now(&repo, LARGE_MB)
        .into_iter()
        .inspect(|f| assert_eq!(f.reason, FlagReason::JunkFolder))
        .map(|f| f.path)
        .collect();
    paths.sort();
    assert_eq!(paths, ["a/dist", "src/venv", "target", "web/node_modules"]);
}

#[test]
fn scan_works_on_any_change_list() {
    let repo = TestRepo::new();
    repo.write("later/.env", "A=1\n");
    repo.write("later/ok.txt", "ok\n");
    let project = open(&repo);

    let flags = scan(
        &project,
        &[
            change("later/.env", ChangeKind::New),
            change("later/ok.txt", ChangeKind::New),
            change("gone/missing.pem", ChangeKind::New), // not on disk any more
        ],
        LARGE_MB,
    )
    .unwrap();

    assert_eq!(
        flags,
        [
            flag("later/.env", FlagReason::SecretName, false, false),
            flag("gone/missing.pem", FlagReason::SecretName, false, false),
        ]
    );
}

#[test]
fn apply_leave_out_ignores_a_new_file() {
    let repo = TestRepo::new();
    repo.write(".env", "PORT=3000\n");
    let project = open(&repo);
    let flags = scan_now(&repo, LARGE_MB);

    let decision = apply_decisions(&project, &[(flags[0].clone(), FlagAction::LeaveOut)]).unwrap();

    assert_eq!(
        decision,
        Decision::Proceed {
            excluded: vec![".env".to_owned()]
        }
    );
    assert_eq!(gitignore(&repo), "/.env\n");
    // Only the .gitignore itself is left to save.
    assert_eq!(
        snapshot(&project).changes,
        [change(".gitignore", ChangeKind::New)]
    );
}

#[test]
fn apply_stop_saving_untracks_a_file_but_keeps_it_on_disk() {
    let repo = TestRepo::new();
    repo.write(".env", "PORT=3000\n");
    repo.commit_all("one");
    repo.write(".env", "PORT=3001\n");
    let project = open(&repo);
    let flags = scan_now(&repo, LARGE_MB);
    assert_eq!(allowed_actions(&flags[0])[0], FlagAction::StopSaving);

    let decision =
        apply_decisions(&project, &[(flags[0].clone(), FlagAction::StopSaving)]).unwrap();

    assert_eq!(
        decision,
        Decision::Proceed {
            excluded: vec![".env".to_owned()]
        }
    );
    assert_eq!(repo.git(&["ls-files", ".env"]), "");
    assert_eq!(
        fs::read_to_string(repo.path().join(".env")).unwrap(),
        "PORT=3001\n"
    );
    assert_eq!(gitignore(&repo), "/.env\n");
}

#[test]
fn apply_leave_out_of_a_junk_folder_ignores_it_by_name() {
    let repo = TestRepo::new();
    repo.write("web/node_modules/a/index.js", "a\n");
    let project = open(&repo);
    let flags = scan_now(&repo, LARGE_MB);

    let decision = apply_decisions(&project, &[(flags[0].clone(), FlagAction::LeaveOut)]).unwrap();

    assert_eq!(
        decision,
        Decision::Proceed {
            excluded: vec!["web/node_modules".to_owned()]
        }
    );
    assert_eq!(gitignore(&repo), "node_modules/\n");
}

#[test]
fn apply_stop_saving_a_tracked_junk_folder_untracks_all_of_it() {
    let repo = TestRepo::new();
    repo.write("web/node_modules/a/index.js", "a\n");
    repo.write("web/app.js", "app\n");
    repo.commit_all("one");
    repo.write("web/node_modules/a/index.js", "b\n");
    let project = open(&repo);
    let flags = scan_now(&repo, LARGE_MB);

    apply_decisions(&project, &[(flags[0].clone(), FlagAction::StopSaving)]).unwrap();

    assert_eq!(repo.git(&["ls-files"]), "web/app.js\n");
    assert!(repo.path().join("web/node_modules/a/index.js").is_file());
    assert_eq!(gitignore(&repo), "node_modules/\n");
}

#[test]
fn leave_out_unstages_a_staged_new_file() {
    // A failed save leaves everything staged by `git add -A`; the retry sees `.env` as new.
    let repo = TestRepo::new();
    repo.write(".env", "PORT=3000\n");
    repo.git(&["add", ".env"]);
    let project = open(&repo);
    let flags = scan_now(&repo, LARGE_MB);
    assert_eq!(flags, [flag(".env", FlagReason::SecretName, false, false)]);

    let decision = apply_decisions(&project, &[(flags[0].clone(), FlagAction::LeaveOut)]).unwrap();

    assert_eq!(
        decision,
        Decision::Proceed {
            excluded: vec![".env".to_owned()]
        }
    );
    assert_eq!(repo.git(&["ls-files", "--cached", ".env"]), "");
    assert!(repo.path().join(".env").is_file());
    assert_eq!(gitignore(&repo), "/.env\n");
    // The save that follows must not pick it up again.
    repo.git(&["add", "-A"]);
    assert_eq!(repo.git(&["ls-files", "--cached"]), ".gitignore\n");
}

#[test]
fn leave_out_unstages_a_staged_junk_folder() {
    let repo = TestRepo::new();
    repo.write("node_modules/a.js", "a\n");
    repo.write("node_modules/deep/b.js", "b\n");
    repo.write("app.js", "app\n");
    repo.git(&["add", "-A"]);
    let project = open(&repo);
    let flags = scan_now(&repo, LARGE_MB);
    assert_eq!(
        flags,
        [flag("node_modules", FlagReason::JunkFolder, false, false)]
    );

    apply_decisions(&project, &[(flags[0].clone(), FlagAction::LeaveOut)]).unwrap();

    assert_eq!(repo.git(&["ls-files", "--cached", "node_modules"]), "");
    assert!(repo.path().join("node_modules/deep/b.js").is_file());
    assert_eq!(gitignore(&repo), "node_modules/\n");
    repo.git(&["add", "-A"]);
    assert_eq!(repo.git(&["ls-files", "--cached"]), ".gitignore\napp.js\n");
}

#[test]
fn leave_out_untracks_the_saved_part_of_a_junk_folder() {
    // Only a new file under `dist` changed, so the folder reads as not yet saved, yet an
    // earlier save holds `dist/a.js`.
    let repo = TestRepo::new();
    repo.write("dist/a.js", "a\n");
    repo.commit_all("one");
    repo.write("dist/new.js", "n\n");
    let project = open(&repo);
    let flags = scan_now(&repo, LARGE_MB);
    assert!(!flags[0].already_saved);

    apply_decisions(&project, &[(flags[0].clone(), FlagAction::LeaveOut)]).unwrap();

    assert_eq!(repo.git(&["ls-files", "--cached", "dist"]), "");
    assert!(repo.path().join("dist/a.js").is_file());
}

#[test]
fn stop_saving_works_when_the_index_differs_from_the_file_and_the_last_save() {
    let repo = TestRepo::new();
    repo.write(".env", "PORT=1\n");
    repo.commit_all("one");
    repo.write(".env", "PORT=2\n");
    repo.git(&["add", ".env"]);
    repo.write(".env", "PORT=3\n");
    let project = open(&repo);
    let flags = scan_now(&repo, LARGE_MB);
    assert_eq!(flags, [flag(".env", FlagReason::SecretName, true, true)]);

    let decision =
        apply_decisions(&project, &[(flags[0].clone(), FlagAction::StopSaving)]).unwrap();

    assert_eq!(
        decision,
        Decision::Proceed {
            excluded: vec![".env".to_owned()]
        }
    );
    assert_eq!(repo.git(&["ls-files", "--cached", ".env"]), "");
    assert_eq!(
        fs::read_to_string(repo.path().join(".env")).unwrap(),
        "PORT=3\n"
    );
    assert_eq!(gitignore(&repo), "/.env\n");
}

#[test]
fn apply_leave_out_of_a_large_file_ignores_that_exact_path() {
    let repo = TestRepo::new();
    repo.write("media/[clip].bin", vec![0u8; 2 * MB]);
    let project = open(&repo);
    let flags = scan_now(&repo, 1);

    apply_decisions(&project, &[(flags[0].clone(), FlagAction::LeaveOut)]).unwrap();

    // Brackets are escaped so the line names this file, not a pattern.
    assert_eq!(gitignore(&repo), "/media/\\[clip].bin\n");
    assert_eq!(
        snapshot(&project).changes,
        [change(".gitignore", ChangeKind::New)]
    );
}

#[test]
fn apply_cancel_and_fix_returns_the_path_and_changes_nothing() {
    let repo = TestRepo::new();
    repo.write(".env", "A=1\n");
    repo.write("src/api.js", format!("const k = \"{}\";\n", aws_key()));
    repo.write("src/other.js", format!("const k = \"{}\";\n", aws_key()));
    let project = open(&repo);
    let flags = scan_now(&repo, LARGE_MB);
    let by_path = |path: &str| flags.iter().find(|f| f.path == path).unwrap().clone();

    let decision = apply_decisions(
        &project,
        &[
            (by_path(".env"), FlagAction::LeaveOut),
            (by_path("src/api.js"), FlagAction::CancelAndFix),
            (by_path("src/other.js"), FlagAction::CancelAndFix),
        ],
    )
    .unwrap();

    assert_eq!(
        decision,
        Decision::CancelAndFix {
            path: "src/api.js".to_owned()
        }
    );
    assert!(!repo.path().join(".gitignore").exists());
}

#[test]
fn apply_save_anyway_leaves_the_repo_alone() {
    let repo = TestRepo::new();
    repo.write(".env", "A=1\n");
    repo.write("node_modules/a.js", "a\n");
    let project = open(&repo);
    let before = repo.git(&["status", "--porcelain=v2"]);
    let flags = scan_now(&repo, LARGE_MB);
    let decisions: Vec<_> = flags
        .into_iter()
        .map(|f| (f, FlagAction::SaveAnyway))
        .collect();

    let decision = apply_decisions(&project, &decisions).unwrap();

    assert_eq!(decision, Decision::Proceed { excluded: vec![] });
    assert_eq!(repo.git(&["status", "--porcelain=v2"]), before);
    assert!(!repo.path().join(".gitignore").exists());
}

#[test]
fn apply_handles_several_decisions_in_order() {
    let repo = TestRepo::new();
    repo.write(".env", "A=1\n");
    repo.write("node_modules/a.js", "a\n");
    repo.write("server.pem", "pem\n");
    let project = open(&repo);
    let flags = scan_now(&repo, LARGE_MB);
    let decisions: Vec<_> = flags
        .into_iter()
        .map(|f| {
            let action = if f.path == "server.pem" {
                FlagAction::SaveAnyway
            } else {
                FlagAction::LeaveOut
            };
            (f, action)
        })
        .collect();

    let decision = apply_decisions(&project, &decisions).unwrap();

    let Decision::Proceed { mut excluded } = decision else {
        panic!("expected Proceed");
    };
    excluded.sort();
    assert_eq!(excluded, [".env", "node_modules"]);
    let mut lines: Vec<_> = gitignore(&repo).lines().map(str::to_owned).collect();
    lines.sort();
    assert_eq!(lines, ["/.env", "node_modules/"]);
}

#[test]
fn gitignore_append_starts_on_a_new_line() {
    let repo = TestRepo::empty_dir();
    fs::write(repo.path().join(".gitignore"), "a").unwrap();

    append_lines(repo.path(), &["/.env".to_owned()]).unwrap();

    assert_eq!(gitignore(&repo), "a\n/.env\n");
}

#[test]
fn gitignore_append_creates_the_file_and_skips_lines_it_has() {
    let repo = TestRepo::empty_dir();
    let lines = vec!["/.env".to_owned(), "node_modules/".to_owned()];

    append_lines(repo.path(), &lines).unwrap();
    assert_eq!(gitignore(&repo), "/.env\nnode_modules/\n");

    // The same lines again, a repeated line, and one new line.
    let more = vec!["/.env".to_owned(), "dist/".to_owned(), "dist/".to_owned()];
    append_lines(repo.path(), &more).unwrap();
    assert_eq!(gitignore(&repo), "/.env\nnode_modules/\ndist/\n");
}

#[test]
fn gitignore_append_keeps_a_file_that_already_ends_in_a_newline_tidy() {
    let repo = TestRepo::empty_dir();
    fs::write(repo.path().join(".gitignore"), "a\r\nb\r\n").unwrap();

    append_lines(repo.path(), &["b".to_owned(), "c".to_owned()]).unwrap();

    assert_eq!(gitignore(&repo), "a\r\nb\r\nc\n");
}

#[test]
fn gitignore_append_with_nothing_to_add_does_not_create_the_file() {
    let repo = TestRepo::empty_dir();

    append_lines(repo.path(), &[]).unwrap();

    assert!(!repo.path().join(".gitignore").exists());
}
