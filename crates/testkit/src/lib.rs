//! Test helpers for crates that drive git. Dev-dependency only, never shipped.
//!
//! Unlike every shipped crate this one does not forbid `unsafe_code`: [`isolate`] must call
//! `std::env::set_var`, which is `unsafe` in edition 2024.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Once;

use savestates_git::{GitExe, Repo, RunOpts, locate, run_in};
use tempfile::TempDir;

static ISOLATE: Once = Once::new();

/// Points git at a per-process global config and disables the system one, so tests never
/// read or write the developer's real git configuration. Runs once per process.
///
/// The global file sets `user.useConfigOnly` so a repository without a configured identity
/// fails to commit instead of git guessing one from the OS user name and host name.
fn isolate() {
    ISOLATE.call_once(|| {
        let config = std::env::temp_dir().join(format!(
            "savestates-testkit-{}.gitconfig",
            std::process::id()
        ));
        fs::write(&config, "[user]\n\tuseConfigOnly = true\n")
            .expect("write isolated global git config");
        // SAFETY: runs exactly once, inside `Once`, before any test spawns git. Windows
        // environment access is serialized by the OS, and testkit is exempt from `forbid(unsafe_code)`.
        unsafe {
            std::env::set_var("GIT_CONFIG_GLOBAL", &config);
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        }
    });
}

/// A temporary directory, normally holding a git repository, deleted on drop.
pub struct TestRepo {
    dir: TempDir,
    exe: GitExe,
}

impl TestRepo {
    /// A fresh repository on branch `main` with a local identity and `core.autocrlf=false`.
    pub fn new() -> Self {
        let repo = Self::new_without_identity();
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo
    }

    /// Like [`TestRepo::new`] but with no `user.name`/`user.email`, so commits fail.
    pub fn new_without_identity() -> Self {
        let repo = Self::empty_dir();
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "core.autocrlf", "false"]);
        repo
    }

    /// A temporary directory that is not a git repository.
    pub fn empty_dir() -> Self {
        isolate();
        let exe = locate().expect("a supported git must be installed");
        Self {
            dir: TempDir::new().expect("create temp dir"),
            exe,
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Writes `bytes` to `rel` (relative to the root), creating parent directories.
    pub fn write(&self, rel: &str, bytes: impl AsRef<[u8]>) {
        let path = self.path().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directories");
        }
        fs::write(&path, bytes).unwrap_or_else(|e| panic!("write {rel}: {e}"));
    }

    /// Removes the file or directory at `rel`.
    pub fn remove(&self, rel: &str) {
        let path: PathBuf = self.path().join(rel);
        let result = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        result.unwrap_or_else(|e| panic!("remove {rel}: {e}"));
    }

    /// Runs git in the root and returns its stdout. Panics if git fails.
    pub fn git(&self, args: &[&str]) -> String {
        let out = run_in(&self.exe, self.path(), args, &RunOpts::default())
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Stages everything, commits it with `msg` verbatim, and returns the new commit's oid.
    pub fn commit_all(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--cleanup=verbatim", "-m", msg]);
        self.git(&["rev-parse", "HEAD"]).trim().to_owned()
    }

    pub fn repo(&self) -> Repo {
        Repo {
            exe: self.exe.clone(),
            root: self.path().to_owned(),
        }
    }

    pub fn exe(&self) -> GitExe {
        self.exe.clone()
    }
}

impl Default for TestRepo {
    fn default() -> Self {
        Self::new()
    }
}
