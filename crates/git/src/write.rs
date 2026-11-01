//! Locating and creating repositories.

use std::path::{Path, PathBuf};

use crate::runner::{GitError, GitExe, RunOpts, run_in};

/// Initialises a repository in `path` (which must exist) whose first branch is `main`.
pub fn init(exe: &GitExe, path: &Path) -> Result<(), GitError> {
    run_in(exe, path, &["init", "-b", "main"], &RunOpts::default())?;
    Ok(())
}

/// True when the repository found from `path` is bare.
pub fn is_bare(exe: &GitExe, path: &Path) -> Result<bool, GitError> {
    let out = run_in(
        exe,
        path,
        &["rev-parse", "--is-bare-repository"],
        &read_only(),
    )?;
    Ok(String::from_utf8_lossy(&out).trim() == "true")
}

/// The root of the work tree containing `path`, in the OS's own path form.
pub fn toplevel(exe: &GitExe, path: &Path) -> Result<PathBuf, GitError> {
    let out = run_in(exe, path, &["rev-parse", "--show-toplevel"], &read_only())?;
    // Git prints forward slashes on Windows (`C:/Users/...`); canonicalising gives the native form.
    let printed = PathBuf::from(String::from_utf8_lossy(&out).trim_end_matches(['\r', '\n']));
    Ok(dunce::canonicalize(printed)?)
}

fn read_only<'a>() -> RunOpts<'a> {
    RunOpts {
        read_only: true,
        ..RunOpts::default()
    }
}
