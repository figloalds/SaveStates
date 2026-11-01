//! Setting work aside on a private ref, so it stays out of the user's own stash list, and
//! putting it back.

use crate::runner::{GitError, Repo, RunOpts};

/// Who the commits behind a recovery snapshot are written by. Passed on the command line so
/// setting work aside does not depend on the user having configured a git identity.
const RECOVERY_IDENTITY: &[(&str, &str)] = &[
    ("user.name", "SaveStates"),
    ("user.email", "savestates@localhost"),
];

const STASH_MESSAGE: &str = "SaveStates recovery";

/// What `git stash push` says when it finds nothing to put in the stash.
const NOTHING_TO_STASH: &str = "No local changes to save";

/// What a snapshot takes.
enum Selection<'a> {
    /// All changes, and the new files that are not ignored.
    Everything,
    /// The same, limited to one file (or folder), taken literally.
    Path(&'a str),
    /// Only these files, taken literally, ignored ones included.
    Files(&'a [String]),
}

/// Sets the work tree's changes aside, including new files that are not ignored, and leaves a
/// private ref `ref_name` pointing at them. `path` limits this to one file (or folder), taken
/// literally. The folder ends up without those changes, and the user's own stash list as it was.
///
/// Returns the commit the ref points at, or `None` when there was nothing to set aside (in
/// which case no ref is created). The ref is made before the stash entry is dropped, so an error
/// part-way leaves the work in the stash list rather than nowhere.
///
/// When git fails part-way, after it made the entry (a file in use that it cannot remove, say),
/// the entry is still moved to `ref_name` and the error is returned. The caller can find the
/// snapshot with [`refs_with_prefix`].
pub fn stash_snapshot_to_ref(
    repo: &Repo,
    path: Option<&str>,
    ref_name: &str,
) -> Result<Option<String>, GitError> {
    let selection = path.map_or(Selection::Everything, Selection::Path);
    snapshot_to_ref(repo, &selection, ref_name)
}

/// Like [`stash_snapshot_to_ref`] for exactly the files in `paths`, taken literally, which may be
/// ignored files too (`stash push --all`). Nothing else is touched. `None` for an empty list.
pub fn stash_snapshot_files_to_ref(
    repo: &Repo,
    paths: &[String],
    ref_name: &str,
) -> Result<Option<String>, GitError> {
    if paths.is_empty() {
        return Ok(None);
    }
    snapshot_to_ref(repo, &Selection::Files(paths), ref_name)
}

fn snapshot_to_ref(
    repo: &Repo,
    selection: &Selection,
    ref_name: &str,
) -> Result<Option<String>, GitError> {
    // `stash push -q` prints nothing at all when there is nothing to stash, so the only
    // reliable sign that it made an entry is the top of the stash list having changed.
    let before = resolve(repo, "refs/stash")?;

    let mut args = Vec::new();
    if !matches!(selection, Selection::Everything) {
        // Not for the whole tree: git hands `:/` to its own `git clean`, and read literally it
        // names no file, so the new files would be stashed but left in the folder.
        args.push("--literal-pathspecs");
    }
    args.extend(["stash", "push"]);
    args.push(match selection {
        Selection::Files(_) => "--all",
        Selection::Everything | Selection::Path(_) => "--include-untracked",
    });
    args.extend(["-q", "-m", STASH_MESSAGE]);
    // The file names go in on standard input: there may be too many for a command line.
    let mut names = Vec::new();
    match selection {
        Selection::Everything => {}
        Selection::Path(path) => args.extend(["--", path]),
        Selection::Files(paths) => {
            args.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
            for path in *paths {
                names.extend_from_slice(path.as_bytes());
                names.push(0);
            }
        }
    }
    let opts = RunOpts {
        config: RECOVERY_IDENTITY,
        stdin: (!names.is_empty()).then_some(names.as_slice()),
        ..RunOpts::default()
    };
    let pushed = repo.run(&args, &opts);
    match &pushed {
        Ok(out) if String::from_utf8_lossy(out).contains(NOTHING_TO_STASH) => return Ok(None),
        Err(GitError::Failed { stderr, .. }) if stderr.contains(NOTHING_TO_STASH) => {
            return Ok(None);
        }
        _ => {}
    }

    // Also when git failed: it may have made the entry before it did. If so the work is in the
    // user's stash list now, which is the wrong place for it.
    let Some(oid) = resolve(repo, "refs/stash")? else {
        return pushed.map(|_| None);
    };
    if before.as_deref() == Some(oid.as_str()) {
        return pushed.map(|_| None);
    }
    if resolve(repo, "stash@{0}")?.as_deref() != Some(oid.as_str()) {
        // Left where it is: the entry stays in the stash list, so the work is not lost.
        return Err(GitError::Failed {
            args: "stash push".to_owned(),
            code: None,
            stderr: "the new stash entry is not at the top of the stash list".to_owned(),
        });
    }

    // Referenced first, dropped second: at no point is the snapshot unreachable.
    update_ref(repo, ref_name, &oid)?;
    repo.run(&["stash", "drop", "-q", "stash@{0}"], &RunOpts::default())?;
    pushed.map(|_| Some(oid))
}

/// Makes the ref `name` point at the object `oid`, creating the ref if needed.
pub fn update_ref(repo: &Repo, name: &str, oid: &str) -> Result<(), GitError> {
    repo.run(&["update-ref", name, oid], &RunOpts::default())?;
    Ok(())
}

/// Deletes the ref `name`.
pub fn delete_ref(repo: &Repo, name: &str) -> Result<(), GitError> {
    repo.run(&["update-ref", "-d", name], &RunOpts::default())?;
    Ok(())
}

/// Every ref whose name starts with `prefix` (a ref path ending in `/`, such as
/// `refs/savestates/recovery/`), as `(name, oid)`, in git's order.
pub fn refs_with_prefix(repo: &Repo, prefix: &str) -> Result<Vec<(String, String)>, GitError> {
    let out = repo.run(
        &["for-each-ref", "--format=%(refname) %(objectname)", prefix],
        &RunOpts::read_only(),
    )?;
    // Ref names cannot contain spaces or line breaks, so both separators are safe.
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, oid)| (name.to_owned(), oid.to_owned()))
        .collect())
}

/// Applies the changes held by the stash-shaped commit `oid` to the work tree (`git stash
/// apply`). It does not have to be in the stash list. An `Err(GitError::Failed)` means git
/// refused or hit a conflict, and may have left the work tree part-way applied.
pub fn stash_apply(repo: &Repo, oid: &str) -> Result<(), GitError> {
    repo.run(&["stash", "apply", "-q", oid], &RunOpts::default())?;
    Ok(())
}

/// Makes the files at `pathspec` in the work tree match their state in `source` (a commit),
/// leaving the index alone (`git restore --source=<source> --worktree`). Tracked files that
/// `source` does not have are removed. `pathspec` is passed to git as is, so it may use
/// pathspec magic such as `:/` or `:(top,literal)<path>`.
pub fn restore_worktree(repo: &Repo, source: &str, pathspec: &str) -> Result<(), GitError> {
    let source = format!("--source={source}");
    repo.run(
        &["restore", &source, "--worktree", "--", pathspec],
        &RunOpts::default(),
    )?;
    Ok(())
}

/// The object `rev` names, or `None` when it names nothing.
fn resolve(repo: &Repo, rev: &str) -> Result<Option<String>, GitError> {
    match repo.run(&["rev-parse", "-q", "--verify", rev], &RunOpts::read_only()) {
        Ok(out) => Ok(Some(String::from_utf8_lossy(&out).trim().to_owned())),
        // `--quiet` makes an unresolvable name a silent exit code 1.
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(e) => Err(e),
    }
}
