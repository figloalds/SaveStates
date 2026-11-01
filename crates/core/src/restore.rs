//! Bringing the past back: a file, or the whole project, copied from an old save into the project
//! folder as unsaved changes. Saves, and the timeline, stay as they are.

use std::fs;
use std::io;

use savestates_git::{blob_size, first_parent, ls_tree_names, restore_worktree, status};

use crate::error::CoreError;
use crate::model::SaveId;
use crate::project::Project;
use crate::recovery::{
    RecoveryEntry, Scope, ignores_case, set_aside_ignored_among, set_aside_ignored_file, throw_away,
};
use crate::timeline::{Side, stays_inside_project};

/// What [`bring_back_everything`] does about unsaved changes in the folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsavedPolicy {
    /// Refuse, and touch nothing.
    RequireClean,
    /// Throw them away first, recoverably.
    MoveToRecovery,
}

#[derive(thiserror::Error, Debug)]
pub enum BringBackError {
    #[error("there are unsaved changes")]
    HasUnsavedChanges,
    #[error(transparent)]
    Core(#[from] CoreError),
}

/// Copies `path` as it was on `side` (a save, or just before it) into the project folder, where
/// it shows up as an unsaved change. A file that did not exist there is deleted from the folder
/// instead. Nothing is staged or saved.
///
/// What is in the folder at that path is set aside first, and the recovery entry is returned so
/// the caller can offer to undo: unsaved changes are thrown away, and an ignored file (which git
/// does not list as a change, and which would be overwritten or deleted) is snapshotted. A path
/// spelled in another case than git reports it is found on a case-insensitive project.
pub fn bring_back_file(
    p: &Project,
    side: &Side,
    path: &str,
) -> Result<Option<RecoveryEntry>, CoreError> {
    // The path is joined to the project folder to delete a file, so it must stay inside it.
    if !stays_inside_project(path) {
        return Err(CoreError::NotFound(path.to_owned()));
    }
    let source = match side {
        Side::Now => return Err(CoreError::NotFound("now".to_owned())),
        Side::Save(id) => Some(id.0.clone()),
        // A first save has nothing before it.
        Side::Before(id) => first_parent(p.repo(), &id.0)?,
    };
    // Looked up before anything is thrown away, so a bad save id costs the user nothing.
    let source_with_file = match source {
        Some(rev) if blob_size(p.repo(), &rev, path)?.is_some() => Some(rev),
        _ => None,
    };

    // At most one of these applies: a file is either listed as a change or ignored and untracked.
    let ignored_set_aside = set_aside_ignored_file(p, path)?;
    let changes_set_aside = match unsaved_spelling_of(p, path)? {
        Some(spelled) => throw_away(p, &Scope::File(spelled))?,
        None => None,
    };

    match source_with_file {
        // Literal and from the project's top, whatever the name holds (`[id].js`, accents).
        Some(rev) => restore_worktree(p.repo(), &rev, &format!(":(top,literal){path}"))?,
        None => match fs::remove_file(p.root().join(path)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        },
    }
    Ok(changes_set_aside.or(ignored_set_aside))
}

/// Makes the project folder match the save `id`: files are restored, added back and removed.
/// The result shows up as unsaved changes; a Quick Save then records it as a new save.
///
/// Unsaved changes are handled as `policy` says. Ignored files are left alone, except those at
/// paths the save has a file for, which would be overwritten: git does not list them as changes,
/// so they are snapshotted whatever the policy (never the rest of what is ignored, such as
/// `node_modules`).
///
/// Returns the most recent recovery entry made: the one for the unsaved changes if they were
/// moved, else the one for the ignored files. When both were made, the other is in
/// `list_recovery`.
pub fn bring_back_everything(
    p: &Project,
    id: &SaveId,
    policy: UnsavedPolicy,
) -> Result<Option<RecoveryEntry>, BringBackError> {
    let has_changes = !status(p.repo())
        .map_err(CoreError::from)?
        .entries
        .is_empty();
    if has_changes && policy == UnsavedPolicy::RequireClean {
        return Err(BringBackError::HasUnsavedChanges);
    }

    let in_save = ls_tree_names(p.repo(), &id.0).map_err(CoreError::from)?;
    // Ignored files first, so that the entry for the unsaved changes is the more recent one.
    let ignored_set_aside = set_aside_ignored_among(p, &in_save)?;
    let changes_set_aside = if has_changes {
        throw_away(p, &Scope::All)?
    } else {
        None
    };
    restore_worktree(p.repo(), &id.0, ":/").map_err(CoreError::from)?;
    Ok(changes_set_aside.or(ignored_set_aside))
}

/// The way git spells `path` in its list of changes (it can be the old name of a rename), or
/// `None` when it is not listed. On a case-insensitive project `readme.md` finds `README.md`.
fn unsaved_spelling_of(p: &Project, path: &str) -> Result<Option<String>, CoreError> {
    let fold_case = ignores_case(p)?;
    let same = |other: &str| {
        if fold_case {
            other.to_lowercase() == path.to_lowercase()
        } else {
            other == path
        }
    };
    Ok(status(p.repo())?.entries.into_iter().find_map(|entry| {
        if same(&entry.path) {
            Some(entry.path)
        } else {
            entry.orig_path.filter(|old| same(old))
        }
    }))
}
