use std::fs;

use savestates_core::{
    AI_PATCH_CAP, Flag, FlagAction, FlagReason, OpenOutcome, Project, SaveError, SaveId,
    apply_decisions, commit_all, hooks_configured, identity_missing, naming_input, normalize_name,
    open_project, set_identity_if_missing,
};
use savestates_git::{GitError, rm_cached};
use testkit::TestRepo;

const LARGE_MB: u64 = 25;
const TRUNCATION_MARK: &str = "\n[... diff truncated ...]";

fn open(repo: &TestRepo) -> Project {
    match open_project(&repo.exe(), repo.path()).unwrap() {
        OpenOutcome::Opened(project) => project,
        other => panic!("expected Opened, got {other:?}"),
    }
}

fn paths(list: &[&str]) -> Vec<String> {
    list.iter().map(|path| (*path).to_owned()).collect()
}

/// The names of the files in the latest save, one per line.
fn files_in_head(repo: &TestRepo) -> Vec<String> {
    repo.git(&["show", "--name-only", "--format=", "HEAD"])
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn identity_detection_and_global_write() {
    // The only test in this file that touches the process-wide global git configuration; the
    // steps below stay in one test so no other test sees them half done.
    let repo = TestRepo::new_without_identity();
    let project = open(&repo);
    assert!(identity_missing(&project).unwrap());

    set_identity_if_missing(&repo.exe(), "Ana", "ana@example.com").unwrap();
    assert!(!identity_missing(&project).unwrap());

    set_identity_if_missing(&repo.exe(), "Bia", "bia@example.com").unwrap();
    assert_eq!(
        repo.git(&["config", "--global", "user.name"]).trim(),
        "Ana",
        "a name that is already set is left alone"
    );
    assert_eq!(
        repo.git(&["config", "--global", "user.email"]).trim(),
        "ana@example.com"
    );

    // Only the key that is unset gets written.
    repo.git(&["config", "--global", "--unset", "user.email"]);
    set_identity_if_missing(&repo.exe(), "Cris", "cris@example.com").unwrap();
    assert_eq!(repo.git(&["config", "--global", "user.name"]).trim(), "Ana");
    assert_eq!(
        repo.git(&["config", "--global", "user.email"]).trim(),
        "cris@example.com"
    );

    // A key that is set but empty counts as unset.
    repo.git(&["config", "--global", "user.name", ""]);
    set_identity_if_missing(&repo.exe(), "Duda", "duda@example.com").unwrap();
    assert_eq!(
        repo.git(&["config", "--global", "user.name"]).trim(),
        "Duda"
    );
    assert_eq!(
        repo.git(&["config", "--global", "user.email"]).trim(),
        "cris@example.com"
    );
}

#[test]
fn naming_input_leaves_index_alone() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    repo.write("b.txt", "bee\n");
    repo.commit_all("first");
    repo.write("a.txt", "one\ntwo\n");
    repo.write("new.txt", "brand new text\n");
    repo.write("staged.txt", "already staged\n");
    repo.git(&["add", "staged.txt"]);
    // Let git refresh the index once, so the comparison below is not about a stat refresh.
    repo.git(&["status"]);

    let status_before = repo.git(&["status", "--porcelain=v1"]);
    let index_before = fs::read(repo.path().join(".git/index")).unwrap();

    let input = naming_input(&open(&repo), &[]).unwrap();

    assert!(input.stat.contains("a.txt"), "{}", input.stat);
    assert!(input.stat.contains("new.txt"), "{}", input.stat);
    assert!(input.stat.contains("staged.txt"), "{}", input.stat);
    assert!(input.patch.contains("brand new text"), "{}", input.patch);
    assert!(input.patch.contains("+two"), "{}", input.patch);
    assert_eq!(repo.git(&["status", "--porcelain=v1"]), status_before);
    assert_eq!(
        fs::read(repo.path().join(".git/index")).unwrap(),
        index_before,
        "the real index must not change"
    );
}

/// Token-shaped test strings, never real keys.
const ANT_KEY: &str = "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789";
const AWS_KEY: &str = "AKIAABCDEFGHIJKLMNOP";

#[test]
fn naming_input_redacts_secrets_on_removed_and_unchanged_lines() {
    // The safety scan only looks at added lines; the AI also sees removed lines and three
    // unchanged lines around each change, so those are cleaned too.
    let repo = TestRepo::new();
    repo.write(
        "config.js",
        format!(
            "const a = 1;\nconst OLD = \"{AWS_KEY}\";\nconst KEY = \"{ANT_KEY}\";\nconst b = 2;\n"
        ),
    );
    repo.commit_all("first");
    // The AWS key goes away (a removed line); the Anthropic key stays next to an edit.
    repo.write(
        "config.js",
        format!("const a = 1;\nconst KEY = \"{ANT_KEY}\";\nconst b = 3;\n"),
    );

    let input = naming_input(&open(&repo), &[]).unwrap();

    assert!(!input.patch.contains(AWS_KEY), "{}", input.patch);
    assert!(!input.patch.contains("sk-ant-"), "{}", input.patch);
    assert!(
        input.patch.contains("-const OLD = \"[redacted]\";"),
        "{}",
        input.patch
    );
    assert!(
        input.patch.contains(" const KEY = \"[redacted]\";"),
        "{}",
        input.patch
    );
    assert!(input.patch.contains("+const b = 3;"), "{}", input.patch);
}

#[test]
fn naming_input_redacts_a_secret_written_after_the_scan() {
    // Nothing excluded it (the scan the user saw came before the file changed): the key is
    // still not sent.
    let repo = TestRepo::new();
    repo.write("a.txt", "hello\n");
    repo.commit_all("first");
    repo.write("api.js", format!("fetch(url, {{ key: \"{ANT_KEY}\" }});\n"));

    let input = naming_input(&open(&repo), &[]).unwrap();

    assert!(input.stat.contains("api.js"), "{}", input.stat);
    assert!(!input.patch.contains(ANT_KEY), "{}", input.patch);
    assert!(
        input.patch.contains("key: \"[redacted]\""),
        "{}",
        input.patch
    );
}

/// Two lines of a test private key's body, never a real key.
const PEM_BODY: [&str; 3] = [
    "MIIEowIBAAKCAQEAu1SU1LfVLPHCozMxH2Mo4lgOEePzNm0tRgeLezV6ffAt0gunVTLw",
    "7onLRnrq0/IzW7yWR7QkrmBL7jTKEn5u+qKhbwKfBstIs+bMY2Zkp18gnTxKLxoS2tFc",
    "zGkPLPgizskuemMghRniWaoLcyehkd3qqGElvW/VDL5AaWTg0nLVkjRo9z+40RQzuVaE",
];

/// A test private key, as a file holds it.
fn pem() -> String {
    format!(
        "-----BEGIN RSA PRIVATE KEY-----\n{}\n-----END RSA PRIVATE KEY-----\n",
        PEM_BODY.join("\n")
    )
}

/// Fails unless no line of the test key, and nothing of `also`, is in the AI's input.
fn assert_nothing_of_the_key(input: &savestates_core::NamingInput, also: &[&str]) {
    let secrets = PEM_BODY.iter().chain(&["PRIVATE KEY"]).chain(also).copied();
    for secret in secrets {
        assert!(!input.stat.contains(secret), "{secret} in:\n{}", input.stat);
        assert!(
            !input.patch.contains(secret),
            "{secret} in:\n{}",
            input.patch
        );
    }
}

#[test]
fn naming_input_sends_nothing_of_a_saved_key_the_user_stops_saving() {
    let repo = TestRepo::new();
    repo.write("server.key", pem());
    repo.write("a.txt", "one\n");
    repo.commit_all("first");
    repo.write("a.txt", "two\n");
    let project = open(&repo);
    let flag = Flag {
        path: "server.key".to_owned(),
        reason: FlagReason::SecretName,
        already_saved: true,
        in_older_saves: true,
    };
    // "Stop saving this file", the recommended action: out of the index, into .gitignore.
    apply_decisions(&project, &[(flag, FlagAction::StopSaving)]).unwrap();

    // As the flow asks: left out.
    let input = naming_input(&project, &paths(&["server.key"])).unwrap();
    assert!(input.stat.contains("a.txt"), "{}", input.stat);
    assert!(!input.stat.contains("server.key"), "{}", input.stat);
    assert_nothing_of_the_key(&input, &[]);

    // Even when not left out, a file that goes away shows without its content.
    let input = naming_input(&project, &[]).unwrap();
    assert!(input.patch.contains("deleted file"), "{}", input.patch);
    assert_nothing_of_the_key(&input, &[]);
}

#[test]
fn naming_input_sends_nothing_of_a_deleted_key_file() {
    // Deleted files are never flagged, so nothing excludes this one.
    let repo = TestRepo::new();
    repo.write("deploy.pem", pem());
    repo.write("a.txt", "one\n");
    repo.commit_all("first");
    repo.remove("deploy.pem");

    let input = naming_input(&open(&repo), &[]).unwrap();

    assert!(input.stat.contains("deploy.pem"), "{}", input.stat);
    assert!(input.patch.contains("deleted file"), "{}", input.patch);
    assert_nothing_of_the_key(&input, &[]);
}

#[test]
fn naming_input_sends_nothing_of_an_excluded_saved_file_that_changed() {
    // "Save anyway" on a changed .env: it is saved, but the AI is not shown it, old or new.
    let repo = TestRepo::new();
    repo.write(".env", "DB_PASSWORD=oldhunter2\n");
    repo.write("a.txt", "one\n");
    repo.commit_all("first");
    repo.write(".env", "DB_PASSWORD=newhunter3\n");
    repo.write("a.txt", "two\n");

    let input = naming_input(&open(&repo), &paths(&[".env"])).unwrap();

    assert!(input.stat.contains("a.txt"), "{}", input.stat);
    assert!(!input.stat.contains(".env"), "{}", input.stat);
    for secret in ["DB_PASSWORD", "oldhunter2", "newhunter3"] {
        assert!(
            !input.patch.contains(secret),
            "{secret} in:\n{}",
            input.patch
        );
    }
}

#[test]
fn naming_input_blanks_a_private_key_shown_around_a_change() {
    // The unchanged lines around a change can show a key, or only part of it.
    let repo = TestRepo::new();
    let key = pem();
    let key = key.trim_end();
    // The change comes right after the key: the hunk starts inside it.
    repo.write(
        "after.js",
        format!("// chave\nconst PEM = `{key}`;\nconst porta = 80;\n"),
    );
    // The change comes right before the key: the hunk ends inside it.
    repo.write(
        "before.js",
        format!("const porta = 80;\nconst PEM = `{key}`;\n// fim\n"),
    );
    // The whole key fits in the lines around the change.
    repo.write("whole.js", format!("const a = 1;\n{key}\nconst b = 2;\n"));
    repo.commit_all("first");
    repo.write(
        "after.js",
        format!("// chave\nconst PEM = `{key}`;\nconst porta = 81;\n"),
    );
    repo.write(
        "before.js",
        format!("const porta = 81;\nconst PEM = `{key}`;\n// fim\n"),
    );
    repo.write("whole.js", format!("const a = 1;\n{key}\nconst b = 3;\n"));

    let input = naming_input(&open(&repo), &[]).unwrap();

    assert_nothing_of_the_key(&input, &[]);
    for kept in ["+const porta = 81;", "-const porta = 80;", "+const b = 3;"] {
        assert!(
            input.patch.contains(kept),
            "{kept} missing:\n{}",
            input.patch
        );
    }
    assert!(input.patch.contains(" [redacted]"), "{}", input.patch);
}

#[test]
fn naming_input_leaves_out_many_paths_without_a_long_command_line() {
    // More left-out paths than fit on one command line.
    let repo = TestRepo::new();
    repo.write("a.txt", "hello\n");
    repo.write(".env", "TOKEN=shh\n");
    let mut excluded = vec![".env".to_owned()];
    excluded.extend((0..3000).map(|i| format!("certs/old/certificate-{i:05}.pem")));

    let input = naming_input(&open(&repo), &excluded).unwrap();

    assert!(input.stat.contains("a.txt"), "{}", input.stat);
    assert!(input.patch.contains("+hello"), "{}", input.patch);
    assert!(!input.stat.contains(".env"), "{}", input.stat);
    assert!(!input.patch.contains("shh"), "{}", input.patch);
}

#[test]
fn naming_input_excludes() {
    let repo = TestRepo::new();
    repo.write("a.txt", "hello world\n");
    repo.write(".env", "TOKEN=shh\n");

    let input = naming_input(&open(&repo), &paths(&[".env"])).unwrap();

    assert!(input.stat.contains("a.txt"), "{}", input.stat);
    assert!(!input.stat.contains(".env"), "{}", input.stat);
    assert!(!input.patch.contains(".env"), "{}", input.patch);
    assert!(!input.patch.contains("TOKEN=shh"), "{}", input.patch);
}

#[test]
fn naming_input_excludes_a_folder_and_takes_paths_literally() {
    let repo = TestRepo::new();
    repo.write("a.txt", "hello world\n");
    repo.write("node_modules/pkg/index.js", "module.exports = 1;\n");
    repo.write("pages/[id].js", "bracket page\n");
    repo.write("pages/i.js", "plain page\n");

    let input = naming_input(&open(&repo), &paths(&["node_modules", "pages/[id].js"])).unwrap();

    assert!(!input.stat.contains("node_modules"), "{}", input.stat);
    assert!(!input.patch.contains("module.exports"), "{}", input.patch);
    assert!(!input.stat.contains("[id].js"), "{}", input.stat);
    assert!(
        input.stat.contains("i.js"),
        "a path that looks like a pattern must not match other files: {}",
        input.stat
    );
}

#[test]
fn naming_input_empty_repo() {
    let repo = TestRepo::new();
    repo.write("first.txt", "the very first file\n");

    let input = naming_input(&open(&repo), &[]).unwrap();

    assert!(input.stat.contains("first.txt"), "{}", input.stat);
    assert!(
        input.patch.contains("the very first file"),
        "{}",
        input.patch
    );
    assert!(repo.path().join(".git").is_dir());
    assert!(
        !repo.path().join(".git/index").exists(),
        "an empty repository must not gain an index"
    );
}

#[test]
fn naming_input_of_a_clean_project_is_empty() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    repo.commit_all("first");

    let input = naming_input(&open(&repo), &[]).unwrap();

    assert_eq!(input.stat.trim(), "");
    assert_eq!(input.patch, "");
}

#[test]
fn naming_input_truncates() {
    let repo = TestRepo::new();
    for index in 0..10 {
        repo.write(
            &format!("big/file{index}.txt"),
            "line of text\n".repeat(800),
        );
    }

    let input = naming_input(&open(&repo), &[]).unwrap();

    assert!(
        input.patch.len() <= AI_PATCH_CAP + 32,
        "patch is {} bytes",
        input.patch.len()
    );
    assert!(input.patch.ends_with(TRUNCATION_MARK), "{}", input.patch);
    for index in 0..10 {
        assert!(
            input.stat.contains(&format!("file{index}.txt")),
            "stat must list every file: {}",
            input.stat
        );
    }
}

#[test]
fn naming_input_truncates_on_a_char_boundary() {
    let repo = TestRepo::new();
    // Three-byte characters, so a cut at the byte cap can land inside one.
    repo.write("long.txt", "ação ✨ ".repeat(8_000));

    let input = naming_input(&open(&repo), &[]).unwrap();

    assert!(input.patch.ends_with(TRUNCATION_MARK));
    assert!(input.patch.len() <= AI_PATCH_CAP + 32);
    assert!(!input.patch.contains('\u{FFFD}'));
}

#[test]
fn naming_input_small_patch_is_untouched() {
    let repo = TestRepo::new();
    repo.write("a.txt", "short\n");

    let input = naming_input(&open(&repo), &[]).unwrap();

    assert!(!input.patch.contains("diff truncated"));
}

#[test]
fn normalize() {
    assert_eq!(
        normalize_name("  Fixed menu  ").as_deref(),
        Some("Fixed menu")
    );
    assert_eq!(normalize_name("a\nb").as_deref(), Some("a b"));
    assert_eq!(normalize_name("a\r\nb").as_deref(), Some("a  b"));
    assert_eq!(normalize_name("a\tb").as_deref(), Some("a b"));
    assert_eq!(normalize_name("   "), None);
    assert_eq!(normalize_name(" \r\n\t "), None);
    assert_eq!(normalize_name(""), None);
    assert_eq!(
        normalize_name("#1 \"menu\" ✨").as_deref(),
        Some("#1 \"menu\" ✨")
    );
}

#[test]
fn commit_exact_name() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    let name = "#1 \"menu\" ✨ ação";

    // The first save of a project, with no HEAD yet.
    let id = commit_all(&open(&repo), name, &paths(&["a.txt"]), LARGE_MB).unwrap();

    assert_eq!(repo.git(&["log", "-1", "--format=%B"]).trim(), name);
    assert_eq!(
        id,
        SaveId(repo.git(&["rev-parse", "HEAD"]).trim().to_owned())
    );
}

#[test]
fn commit_keeps_leading_hash_and_normalizes_whitespace() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    repo.commit_all("first");
    repo.write("a.txt", "two\n");

    commit_all(
        &open(&repo),
        "  # not a comment\n\tsecond line ",
        &paths(&["a.txt"]),
        LARGE_MB,
    )
    .unwrap();

    assert_eq!(
        repo.git(&["log", "-1", "--format=%B"]).trim(),
        "# not a comment  second line"
    );
}

#[test]
fn commit_saves_everything_including_deletions() {
    let repo = TestRepo::new();
    repo.write("keep.txt", "keep\n");
    repo.write("gone.txt", "gone\n");
    repo.commit_all("first");
    repo.remove("gone.txt");
    repo.write("new/deep.txt", "deep\n");
    let changes = paths(&["gone.txt", "new/deep.txt"]);

    commit_all(&open(&repo), "second", &changes, LARGE_MB).unwrap();

    assert_eq!(repo.git(&["status", "--porcelain=v1"]), "");
    assert_eq!(repo.git(&["ls-files"]).lines().count(), 2);
}

#[test]
fn nothing_and_empty() {
    let repo = TestRepo::new();
    let project = open(&repo);

    // Nothing at all, and no first save yet.
    assert!(matches!(
        commit_all(&project, "x", &[], LARGE_MB),
        Err(SaveError::NothingToSave)
    ));

    repo.write("a.txt", "one\n");
    repo.commit_all("first");

    assert!(matches!(
        commit_all(&project, "x", &[], LARGE_MB),
        Err(SaveError::NothingToSave)
    ));

    repo.write("a.txt", "two\n");
    let result = commit_all(&project, "  ", &paths(&["a.txt"]), LARGE_MB);
    assert!(matches!(result, Err(SaveError::EmptyName)), "{result:?}");
    assert_eq!(
        repo.git(&["diff", "--cached", "--name-only"]),
        "",
        "an empty name must be refused before anything is staged"
    );
}

#[test]
fn hook_rejection() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    repo.write(".git/hooks/pre-commit.sample", "#!/bin/sh\nexit 1\n");
    let project = open(&repo);
    assert!(
        !hooks_configured(&project).unwrap(),
        "a .sample hook does not count"
    );

    repo.write(
        ".git/hooks/pre-commit",
        "#!/bin/sh\necho \"lint failed\"\nexit 1\n",
    );
    assert!(hooks_configured(&project).unwrap());

    let result = commit_all(&project, "one", &paths(&["a.txt"]), LARGE_MB);

    match result {
        Err(SaveError::HookRejected { output }) => {
            assert!(output.contains("lint failed"), "{output}");
        }
        other => panic!("expected HookRejected, got {other:?}"),
    }
    assert_eq!(repo.git(&["branch", "--list"]), "", "no save was made");
}

#[test]
fn hooks_configured_detection() {
    let repo = TestRepo::new();
    let project = open(&repo);
    assert!(
        !hooks_configured(&project).unwrap(),
        "a fresh repository only has the .sample hooks"
    );

    for hook in ["prepare-commit-msg", "commit-msg"] {
        repo.write(&format!(".git/hooks/{hook}"), "#!/bin/sh\nexit 0\n");
        assert!(hooks_configured(&project).unwrap(), "{hook}");
        repo.remove(&format!(".git/hooks/{hook}"));
        assert!(!hooks_configured(&project).unwrap());
    }

    repo.git(&["config", "core.hooksPath", "tools/hooks"]);
    assert!(hooks_configured(&project).unwrap(), "core.hooksPath");
}

#[test]
fn changed_during_save() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    repo.write(".env", "TOKEN=shh\n");
    let project = open(&repo);

    // No first save yet: the secret is unstaged even though there is no HEAD to reset to.
    let result = commit_all(&project, "x", &paths(&["a.txt"]), LARGE_MB);

    match result {
        Err(SaveError::ChangedDuringSave { paths }) => assert_eq!(paths, [".env"]),
        other => panic!("expected ChangedDuringSave, got {other:?}"),
    }
    assert_eq!(
        repo.git(&["diff", "--cached", "--name-only"]).trim(),
        "a.txt"
    );
    assert!(repo.path().join(".env").is_file(), "the file stays on disk");
    assert_eq!(repo.git(&["branch", "--list"]), "", "nothing was saved");
}

#[test]
fn changed_during_save_with_an_earlier_save() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    repo.commit_all("first");
    repo.write("a.txt", "two\n");
    repo.write("b.txt", "bee\n");
    repo.write("keys.pem", "not really a key\n");
    let project = open(&repo);

    let result = commit_all(&project, "x", &paths(&["a.txt"]), LARGE_MB);

    match result {
        Err(SaveError::ChangedDuringSave { paths }) => assert_eq!(paths, ["keys.pem"]),
        other => panic!("expected ChangedDuringSave, got {other:?}"),
    }
    assert!(
        !repo
            .git(&["diff", "--cached", "--name-only"])
            .contains("keys.pem")
    );

    // A file nobody could have flagged is simply part of the save.
    repo.remove("keys.pem");
    commit_all(&project, "y", &paths(&["a.txt"]), LARGE_MB).unwrap();
    assert_eq!(files_in_head(&repo), ["a.txt", "b.txt"]);
}

#[test]
fn unflagged_new_file_is_included() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    let project = open(&repo);
    repo.write("b.txt", "bee\n");

    commit_all(&project, "x", &paths(&["a.txt"]), LARGE_MB).unwrap();

    assert_eq!(files_in_head(&repo), ["a.txt", "b.txt"]);
}

#[test]
fn changed_during_save_flags_content_size_and_folders() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    repo.write(
        "config.js",
        format!("const k = \"AKIA{}\";\n", "ABCDEFGH12345678"),
    );
    repo.write("video.bin", vec![7u8; 2 * 1024 * 1024]);
    repo.write("node_modules/pkg/index.js", "x\n");
    let project = open(&repo);

    let result = commit_all(&project, "x", &paths(&["a.txt"]), 1);

    let flagged = match result {
        Err(SaveError::ChangedDuringSave { mut paths }) => {
            paths.sort();
            paths
        }
        other => panic!("expected ChangedDuringSave, got {other:?}"),
    };
    assert_eq!(flagged, ["config.js", "node_modules", "video.bin"]);
    assert_eq!(
        repo.git(&["diff", "--cached", "--name-only"]).trim(),
        "a.txt",
        "the whole junk folder is unstaged, not only its first file"
    );
}

#[test]
fn files_the_user_already_looked_at_are_not_flagged() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    repo.write(".env", "TOKEN=shh\n");
    repo.write("node_modules/pkg/index.js", "x\n");
    repo.write("dist/app/main.js", "y\n");
    // The file itself, the folder it is under, and a file below a folder all count.
    let checked = paths(&["a.txt", ".env", "node_modules", "dist/app/main.js"]);

    commit_all(&open(&repo), "all of it", &checked, LARGE_MB).unwrap();

    assert_eq!(
        files_in_head(&repo),
        [
            ".env",
            "a.txt",
            "dist/app/main.js",
            "node_modules/pkg/index.js"
        ]
    );
}

#[test]
fn stopping_saving_a_folder_is_not_a_surprise() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    repo.write("node_modules/pkg/index.js", "x\n");
    repo.write("node_modules/pkg/other.js", "z\n");
    repo.commit_all("first");
    repo.write(".gitignore", "node_modules/\n");
    // What "stop saving" leaves behind: the whole folder staged as deleted.
    rm_cached(&repo.repo(), "node_modules", true).unwrap();

    commit_all(&open(&repo), "stop", &paths(&[".gitignore"]), LARGE_MB).unwrap();

    assert_eq!(
        repo.git(&["ls-files"]).lines().collect::<Vec<_>>(),
        [".gitignore", "a.txt"]
    );
}

#[test]
fn locked_repo() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    repo.write(".git/index.lock", "");

    let result = commit_all(&open(&repo), "x", &paths(&["a.txt"]), LARGE_MB);

    assert!(
        matches!(result, Err(SaveError::Git(GitError::Locked))),
        "{result:?}"
    );
}

#[test]
fn locked_repo_with_hooks_is_still_locked() {
    let repo = TestRepo::new();
    repo.write("a.txt", "one\n");
    repo.write(".git/hooks/pre-commit", "#!/bin/sh\nexit 0\n");
    repo.write(".git/index.lock", "");

    let result = commit_all(&open(&repo), "x", &paths(&["a.txt"]), LARGE_MB);

    assert!(
        matches!(result, Err(SaveError::Git(GitError::Locked))),
        "{result:?}"
    );
}
