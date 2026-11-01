//! Quick Save: everything in one action. The UI scans for risky files first (see [`crate::scan`]),
//! asks for a name (typed, or suggested from [`naming_input`]), then calls [`commit_all`].

use std::collections::HashSet;
use std::fs;
use std::io;

use savestates_git::{
    GitError, GitExe, RunOpts, add_all, commit, config_get, config_get_global, config_set_global,
    diff_cached, git_path, rm_cached,
};
use tempfile::TempDir;

use crate::error::CoreError;
use crate::model::{Change, ChangeKind, SaveId};
use crate::project::Project;
use crate::safety::{FlagReason, scan};

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
    /// `git diff --cached`, cut at [`AI_PATCH_CAP`] bytes.
    pub patch: String,
}

/// Describes what a Quick Save would record right now, minus the `excluded` paths.
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
    if !excluded.is_empty() {
        // Recursive, so an excluded folder takes everything under it; literal, so a name with
        // `[` or `*` in it is not read as a pattern.
        let mut args = vec![
            "--literal-pathspecs",
            "rm",
            "--cached",
            "-f",
            "-r",
            "-q",
            "--ignore-unmatch",
            "--",
        ];
        args.extend(excluded.iter().map(String::as_str));
        let opts = RunOpts {
            index_file: Some(&index),
            ..RunOpts::default()
        };
        repo.run(&args, &opts)?;
    }

    let stat = diff_cached(repo, Some(&index), &["--stat=200"])?;
    let mut patch = diff_cached(repo, Some(&index), &[])?;
    if patch.len() > AI_PATCH_CAP {
        patch.truncate(patch.floor_char_boundary(AI_PATCH_CAP));
        patch.push_str(TRUNCATION_MARK);
    }
    Ok(NamingInput { stat, patch })
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
