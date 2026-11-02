//! Quick Save: everything in one action. The UI scans for risky files first (see [`crate::scan`]),
//! asks for a name (typed, or suggested from [`naming_input`]), then calls [`commit_all`].

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

use savestates_git::{
    GitError, GitExe, Repo, RunOpts, add_all, commit, config_get, config_get_global,
    config_set_global, diff_cached, git_path, rm_cached,
};
use tempfile::TempDir;

use crate::error::CoreError;
use crate::model::{Change, ChangeKind, SaveId};
use crate::project::Project;
use crate::safety::{FlagReason, redact_secrets, scan};

/// The most bytes of patch handed to the AI that names a save (20 KB).
pub const AI_PATCH_CAP: usize = 20 * 1024;

/// Added to a patch that was cut at [`AI_PATCH_CAP`].
const TRUNCATION_MARK: &str = "\n[... diff truncated ...]";

/// Why a Quick Save did not produce a save.
#[derive(thiserror::Error, Debug)]
pub enum SaveError {
    #[error("there is nothing to save")]
    NothingToSave,
    #[error("a save needs a name")]
    EmptyName,
    /// A commit hook (`pre-commit`, `prepare-commit-msg` or `commit-msg`) refused the save.
    #[error("a hook refused the save: {output}")]
    HookRejected { output: String },
    /// Files that need a decision appeared after the user looked at the project. They were
    /// taken out of the save, which was not made.
    #[error("files changed while saving: {}", paths.join(", "))]
    ChangedDuringSave { paths: Vec<String> },
    #[error(transparent)]
    Git(#[from] GitError),
}

/// True when `user.name` or `user.email` is empty or unset for this project.
pub fn identity_missing(p: &Project) -> Result<bool, CoreError> {
    for key in ["user.name", "user.email"] {
        if config_get(p.repo(), key)?.is_none_or(|value| value.trim().is_empty()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Writes `name` and `email` to the user's global git configuration, only for the keys that are
/// unset or empty there. A key that already has a value is left alone, and so is a key whose new
/// value is empty.
pub fn set_identity_if_missing(git: &GitExe, name: &str, email: &str) -> Result<(), CoreError> {
    for (key, value) in [("user.name", name), ("user.email", email)] {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if config_get_global(git, key)?.is_none_or(|current| current.trim().is_empty()) {
            config_set_global(git, key, value)?;
        }
    }
    Ok(())
}

/// True when a save here may be refused by a hook: `core.hooksPath` is set, or a `pre-commit`,
/// `prepare-commit-msg` or `commit-msg` hook exists (not just its `.sample`).
pub fn hooks_configured(p: &Project) -> Result<bool, CoreError> {
    Ok(savestates_git::hooks_configured(p.repo())?)
}

/// What an AI needs to suggest a name for the save about to be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamingInput {
    /// `git diff --cached --stat=200`: every file, never shortened.
    pub stat: String,
    /// `git diff --cached`, deleted files without their content, secrets redacted (see
    /// [`naming_input`]), cut at [`AI_PATCH_CAP`] bytes.
    pub patch: String,
}

/// The most bytes of pathspecs put on one git command line. Windows allows 32,767 characters
/// for a whole command line; this leaves room for the rest of it.
const PATHSPEC_BUDGET: usize = 16 * 1024;

/// The `git diff --cached` options for the summary and for the patch. Renames are not looked
/// for, so the files left out never pair up with the ones shown.
const STAT_OPTIONS: &[&str] = &["--no-renames", "--stat=200"];
const PATCH_OPTIONS: &[&str] = &["--no-renames", "--irreversible-delete"];

/// Describes what a Quick Save would record right now, for the AI that suggests its name.
///
/// What it guarantees:
/// * The `excluded` paths (a folder: everything under it) are in neither the summary nor the
///   patch, whatever happened to them (new, changed, deleted, or no longer saved).
/// * A deleted file shows only as deleted, without its content.
/// * On every other line of the patch, added, removed or unchanged, whatever looks like a
///   token is replaced by `[redacted]`, and so is every line of a private key, from its BEGIN
///   line through its END line. This happens before the patch is cut, so no piece of a key is
///   left behind.
///
/// What it cannot guarantee: a secret with no recognizable shape (a plain password) in a file
/// that is not excluded is sent as it is. The caller excludes the files the safety scan flags.
///
/// The changes are staged in a scratch copy of the index, so what is staged for real is left
/// exactly as it was.
pub fn naming_input(p: &Project, excluded: &[String]) -> Result<NamingInput, CoreError> {
    let repo = p.repo();
    let scratch = TempDir::new()?;
    let index = scratch.path().join("index");
    // Start from the real index so anything already staged is part of the picture. Before the
    // first save there is no index to copy, and git builds a new one at this path.
    match fs::copy(git_path(repo, "index")?, &index) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }

    add_all(repo, Some(&index))?;
    // The excluded paths are left out of what git shows, not out of the scratch index: taking
    // a saved file out of an index makes it show as deleted, with all its old content.
    let (stat, patch) = staged_diff(repo, &index, excluded)?;
    let mut patch = redact_secrets(&patch);
    if patch.len() > AI_PATCH_CAP {
        patch.truncate(patch.floor_char_boundary(AI_PATCH_CAP));
        patch.push_str(TRUNCATION_MARK);
    }
    Ok(NamingInput { stat, patch })
}

/// The summary and the patch of what `index` stages, minus the `excluded` paths.
///
/// Normally one `git diff --cached` each, with an `:(exclude,literal)` pathspec per excluded
/// path (literal, so a name with `[` or `*` in it is not read as a pattern). When those would
/// not fit on a command line, the staged paths are listed instead, the excluded ones dropped
/// here, and the rest shown a chunk of paths at a time; the summary then has one total line
/// per chunk.
fn staged_diff(
    repo: &Repo,
    index: &Path,
    excluded: &[String],
) -> Result<(String, String), GitError> {
    let excludes: Vec<String> = excluded
        .iter()
        .map(|path| format!(":(exclude,literal){path}"))
        .collect();
    if pathspec_bytes(&excludes) <= PATHSPEC_BUDGET {
        let mut pathspecs = vec![".".to_owned()];
        pathspecs.extend(excludes);
        let stat = diff_paths(repo, index, STAT_OPTIONS, &pathspecs)?;
        let patch = diff_paths(repo, index, PATCH_OPTIONS, &pathspecs)?;
        return Ok((stat, patch));
    }
    let staged = diff_cached(repo, Some(index), &["--no-renames", "--name-only", "-z"])?;
    let kept: Vec<String> = staged
        .split('\0')
        .filter(|path| !path.is_empty() && !is_under_any(path, excluded))
        .map(|path| format!(":(literal){path}"))
        .collect();
    let (mut stat, mut patch) = (String::new(), String::new());
    for chunk in chunks(&kept, PATHSPEC_BUDGET) {
        stat.push_str(&diff_paths(repo, index, STAT_OPTIONS, chunk)?);
        patch.push_str(&diff_paths(repo, index, PATCH_OPTIONS, chunk)?);
    }
    Ok((stat, patch))
}

/// `git diff --cached <options> -- <pathspecs>` on `index`.
fn diff_paths(
    repo: &Repo,
    index: &Path,
    options: &[&str],
    pathspecs: &[String],
) -> Result<String, GitError> {
    let mut args: Vec<&str> = options.to_vec();
    args.push("--");
    args.extend(pathspecs.iter().map(String::as_str));
    diff_cached(repo, Some(index), &args)
}

/// The command-line bytes `args` take, with a separator each.
fn pathspec_bytes(args: &[String]) -> usize {
    args.iter().map(|arg| arg.len() + 1).sum()
}

/// `args` in consecutive groups of at most `budget` bytes (an argument longer than that is a
/// group of its own).
fn chunks(args: &[String], budget: usize) -> Vec<&[String]> {
    let mut groups = Vec::new();
    let (mut start, mut bytes) = (0, 0);
    for (i, arg) in args.iter().enumerate() {
        let size = arg.len() + 1;
        if i > start && bytes + size > budget {
            groups.push(&args[start..i]);
            (start, bytes) = (i, 0);
        }
        bytes += size;
    }
    if start < args.len() {
        groups.push(&args[start..]);
    }
    groups
}

/// True when `path` is one of `folders_or_files`, or under one of them.
fn is_under_any(path: &str, folders_or_files: &[String]) -> bool {
    folders_or_files.iter().any(|excluded| {
        path == excluded
            || path
                .strip_prefix(excluded.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// The name a save will get: line breaks and tabs become spaces, and the ends are trimmed.
/// `None` when nothing is left.
pub fn normalize_name(raw: &str) -> Option<String> {
    let spaced: String = raw
        .chars()
        .map(|c| {
            if matches!(c, '\r' | '\n' | '\t') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let name = spaced.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// Saves every change in the project under `name`: `git add -A`, then a commit with the name
/// as the message, hooks included.
///
/// `checked` lists the paths the user has already been shown and let through: a file, or a
/// folder standing for everything under it. A staged path outside it that the safety rules
/// would flag (see [`crate::scan`]) is unstaged, and the save is not made but reported as
/// [`SaveError::ChangedDuringSave`], so the user can decide about it.
pub fn commit_all(
    p: &Project,
    name: &str,
    checked: &[String],
    large_file_mb: u64,
) -> Result<SaveId, SaveError> {
    let message = normalize_name(name).ok_or(SaveError::EmptyName)?;
    let repo = p.repo();

    add_all(repo, None)?;
    let staged = staged_changes(p)?;
    if staged.is_empty() {
        return Err(SaveError::NothingToSave);
    }
    guard_against_late_arrivals(p, staged, checked, large_file_mb)?;

    commit(repo, &message).map_err(|error| refused_by_hook_or(p, error))?;

    let head = repo.run(&["rev-parse", "HEAD"], &RunOpts::read_only())?;
    Ok(SaveId(String::from_utf8_lossy(&head).trim().to_owned()))
}

/// What is staged, compared with the last save (or with nothing, before the first one).
/// Renames are not looked for: a moved file is a deletion plus a new file.
fn staged_changes(p: &Project) -> Result<Vec<Change>, GitError> {
    let out = diff_cached(p.repo(), None, &["--name-status", "--no-renames", "-z"])?;
    // Records alternate: a status letter, then the path.
    let mut fields = out.split('\0');
    let mut changes = Vec::new();
    while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
        let kind = match status {
            "A" => ChangeKind::New,
            "D" => ChangeKind::Deleted,
            _ => ChangeKind::Changed,
        };
        changes.push(Change {
            path: path.to_owned(),
            kind,
        });
    }
    Ok(changes)
}

/// Unstages the staged paths the user has not seen that the safety rules flag, and reports them.
fn guard_against_late_arrivals(
    p: &Project,
    staged: Vec<Change>,
    checked: &[String],
    large_file_mb: u64,
) -> Result<(), SaveError> {
    let checked: HashSet<&str> = checked.iter().map(String::as_str).collect();
    let unchecked: Vec<Change> = staged
        .into_iter()
        .filter(|change| !is_checked(&checked, &change.path))
        .collect();
    if unchecked.is_empty() {
        return Ok(());
    }

    let flags = scan(p, &unchecked, large_file_mb).map_err(scan_failure)?;
    if flags.is_empty() {
        return Ok(());
    }
    for flag in &flags {
        // `rm --cached` rather than `reset`: it also works before the first save, when there is
        // no HEAD to reset to.
        rm_cached(p.repo(), &flag.path, flag.reason == FlagReason::JunkFolder)?;
    }
    Err(SaveError::ChangedDuringSave {
        paths: flags.into_iter().map(|flag| flag.path).collect(),
    })
}

/// True when `path`, or a folder above it, is in `checked`.
fn is_checked(checked: &HashSet<&str>, path: &str) -> bool {
    checked.contains(path)
        || path
            .match_indices('/')
            .any(|(slash, _)| checked.contains(&path[..slash]))
}

/// `scan` only reads files and asks git; either way the save failed on something external.
fn scan_failure(error: CoreError) -> SaveError {
    match error {
        CoreError::Git(e) => SaveError::Git(e),
        CoreError::Io(e) => SaveError::Git(GitError::Io(e)),
        other => SaveError::Git(GitError::Io(io::Error::other(other))),
    }
}

/// A failed commit in a project with commit hooks is taken to be a hook saying no, and carries
/// what the hook printed. Any other failure (a lock, a missing repository, ...) stays as it is.
fn refused_by_hook_or(p: &Project, error: GitError) -> SaveError {
    if let GitError::Failed { stderr, .. } = &error
        // If the hooks cannot be looked up, the commit's own error is the better one to report.
        && matches!(savestates_git::hooks_configured(p.repo()), Ok(true))
    {
        return SaveError::HookRejected {
            output: stderr.trim().to_owned(),
        };
    }
    SaveError::Git(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn pathspecs_are_grouped_within_the_budget() {
        // Each argument takes its length plus a separator.
        let list = args(&["aaaa", "bbbb", "cccc", "dddddddddddd", "e"]);
        let groups = chunks(&list, 10);
        assert_eq!(
            groups,
            [&list[0..2], &list[2..3], &list[3..4], &list[4..5]],
            "a group never goes past the budget, and a long argument is a group of its own"
        );
        assert_eq!(chunks(&list, 1000), [&list[..]]);
        assert!(chunks(&[], 10).is_empty());
        assert_eq!(pathspec_bytes(&list[..2]), 10);
    }

    #[test]
    fn an_excluded_folder_takes_what_is_under_it() {
        let excluded = args(&["node_modules", "certs/old.pem"]);
        assert!(is_under_any("node_modules", &excluded));
        assert!(is_under_any("node_modules/lodash/index.js", &excluded));
        assert!(is_under_any("certs/old.pem", &excluded));
        assert!(!is_under_any("node_modules_backup/a.js", &excluded));
        assert!(!is_under_any("certs/old.pem.txt", &excluded));
        assert!(!is_under_any("src/node_modules.js", &excluded));
    }
}
