//! Locating and creating repositories, and the few commands that change what is saved.

use std::path::{Path, PathBuf};

use crate::runner::{GitError, GitExe, Repo, RunOpts, run_in};

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
        &RunOpts::read_only(),
    )?;
    Ok(String::from_utf8_lossy(&out).trim() == "true")
}

/// The root of the work tree containing `path`, in the OS's own path form.
pub fn toplevel(exe: &GitExe, path: &Path) -> Result<PathBuf, GitError> {
    let out = run_in(
        exe,
        path,
        &["rev-parse", "--show-toplevel"],
        &RunOpts::read_only(),
    )?;
    // Git prints forward slashes on Windows (`C:/Users/...`); canonicalising gives the native form.
    let printed = PathBuf::from(String::from_utf8_lossy(&out).trim_end_matches(['\r', '\n']));
    Ok(dunce::canonicalize(printed)?)
}

/// Takes `path` (and, with `recursive`, everything under it) out of the index while leaving the
/// files on disk, so the next save records them as no longer saved. A path that is not in the
/// index is not an error.
///
/// Forced (`-f`): without it git refuses when the staged content differs from both the file and
/// HEAD, which is what a failed save leaves behind. Only the staged copy is dropped; `--cached`
/// never touches the file on disk.
pub fn rm_cached(repo: &Repo, path: &str, recursive: bool) -> Result<(), GitError> {
    let mut args = vec![
        "--literal-pathspecs",
        "rm",
        "--cached",
        "-f",
        "-q",
        "--ignore-unmatch",
    ];
    if recursive {
        args.push("-r");
    }
    args.extend(["--", path]);
    repo.run(&args, &RunOpts::default())?;
    Ok(())
}

/// True when some save reachable from HEAD changed `path` (a file, or a folder through the files
/// under it). False when there is no HEAD yet.
pub fn path_in_history(repo: &Repo, path: &str) -> Result<bool, GitError> {
    match repo.run(
        &["rev-parse", "--verify", "--quiet", "HEAD"],
        &RunOpts::read_only(),
    ) {
        Ok(_) => {}
        // `--quiet` makes an unresolvable HEAD a silent exit code 1.
        Err(GitError::Failed { code: Some(1), .. }) => return Ok(false),
        Err(e) => return Err(e),
    }
    let out = repo.run(
        &["--literal-pathspecs", "rev-list", "-1", "HEAD", "--", path],
        &RunOpts::read_only(),
    )?;
    Ok(!out.iter().all(u8::is_ascii_whitespace))
}
