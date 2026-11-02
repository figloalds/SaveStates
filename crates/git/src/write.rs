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

/// Stages every change in the work tree (`git add -A`): new, changed and deleted files.
///
/// With `index_file` this works on that index instead of the repository's own, so a scratch copy
/// can be built without touching what is staged for real.
pub fn add_all(repo: &Repo, index_file: Option<&Path>) -> Result<(), GitError> {
    let opts = RunOpts {
        index_file,
        ..RunOpts::default()
    };
    repo.run(&["add", "-A"], &opts)?;
    Ok(())
}

/// Commits what is staged with `message` exactly as given (`--cleanup=verbatim`, so a leading
/// `#` is not a comment). Hooks run: this never skips them.
///
/// A hook that rejects the commit fails with [`GitError::Failed`]. Git sends the hook's own
/// output to standard error, so `stderr` holds everything the hook printed.
pub fn commit(repo: &Repo, message: &str) -> Result<(), GitError> {
    repo.run(
        &["commit", "-q", "--cleanup=verbatim", "-m", message],
        &RunOpts::default(),
    )?;
    Ok(())
}

/// What every `git diff --cached` passes, whatever the user's git settings say: no colors (a
/// `color.diff` setting beats `-c color.ui=never`, and color codes hide the line prefixes the
/// AI redaction reads), no external diff program (it prints what it likes, deleted files
/// included), and no text conversion (which may decrypt files, git-crypt for one).
const DIFF_PLAIN: &[&str] = &["--no-color", "--no-ext-diff", "--no-textconv"];

/// Runs `git diff --cached <args>` and returns its standard output, with any invalid UTF-8
/// replaced. `index_file` selects a scratch index instead of the repository's own. Read-only.
/// The output is git's own, never changed by the user's settings (see [`DIFF_PLAIN`]).
pub fn diff_cached(
    repo: &Repo,
    index_file: Option<&Path>,
    args: &[&str],
) -> Result<String, GitError> {
    let mut full = vec!["diff", "--cached"];
    full.extend_from_slice(DIFF_PLAIN);
    full.extend_from_slice(args);
    let opts = RunOpts {
        index_file,
        ..RunOpts::read_only()
    };
    let out = repo.run(&full, &opts)?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// The value of `key` as seen from the repository (local, global and system settings), or `None`
/// when it is not set anywhere.
pub fn config_get(repo: &Repo, key: &str) -> Result<Option<String>, GitError> {
    config_value(&repo.exe, &repo.root, &["config", "--get", key])
}

/// The value of `key` in the user's global git configuration, or `None` when it is not set there.
pub fn config_get_global(exe: &GitExe, key: &str) -> Result<Option<String>, GitError> {
    config_value(
        exe,
        &std::env::temp_dir(),
        &["config", "--global", "--get", key],
    )
}

/// Sets `key` in the user's global git configuration.
pub fn config_set_global(exe: &GitExe, key: &str, value: &str) -> Result<(), GitError> {
    run_in(
        exe,
        &std::env::temp_dir(),
        &["config", "--global", key, value],
        &RunOpts::default(),
    )?;
    Ok(())
}

fn config_value(exe: &GitExe, cwd: &Path, args: &[&str]) -> Result<Option<String>, GitError> {
    match run_in(exe, cwd, args, &RunOpts::read_only()) {
        Ok(out) => Ok(Some(
            String::from_utf8_lossy(&out)
                .trim_end_matches(['\r', '\n'])
                .to_owned(),
        )),
        // `git config --get` exits with 1 when the key has no value.
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The hooks that can turn a commit down.
const COMMIT_HOOKS: &[&str] = &["pre-commit", "prepare-commit-msg", "commit-msg"];

/// True when a commit here may run a hook: `core.hooksPath` is set, or the repository's hooks
/// folder holds a `pre-commit`, `prepare-commit-msg` or `commit-msg` file. The `.sample` files
/// git installs do not count.
pub fn hooks_configured(repo: &Repo) -> Result<bool, GitError> {
    if config_get(repo, "core.hooksPath")?.is_some_and(|path| !path.trim().is_empty()) {
        return Ok(true);
    }
    let hooks = git_path(repo, "hooks")?;
    Ok(COMMIT_HOOKS.iter().any(|name| hooks.join(name).is_file()))
}

/// Where git keeps `name` inside the repository (`git rev-parse --git-path`), as an absolute
/// path. This is a path, not a promise that anything exists there.
pub fn git_path(repo: &Repo, name: &str) -> Result<PathBuf, GitError> {
    let out = repo.run(&["rev-parse", "--git-path", name], &RunOpts::read_only())?;
    let printed = String::from_utf8_lossy(&out);
    // Git prints the path relative to where it ran, which is the repository root.
    Ok(repo.root.join(printed.trim_end_matches(['\r', '\n'])))
}
