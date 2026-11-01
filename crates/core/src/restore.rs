//! Bringing the past back: a file, or the whole project, copied from an old save into the project
//! folder as unsaved changes. Saves, and the timeline, stay as they are.

use std::fs;
use std::io;

use savestates_git::{blob_size, first_parent, restore_worktree, status};

use crate::error::CoreError;
use crate::model::SaveId;
use crate::project::Project;
use crate::recovery::{RecoveryEntry, Scope, throw_away};
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
/// If the file has unsaved changes they are thrown away first and the recovery entry is
/// returned, so the caller can offer to undo.
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

    let set_aside = if has_unsaved_changes_in(p, path)? {
        throw_away(p, &Scope::File(path.to_owned()))?
    } else {
        None
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
    Ok(set_aside)
}

/// Makes the project folder match the save `id`: files are restored, added back and removed.
/// Ignored files are not touched. The result shows up as unsaved changes; a Quick Save then
/// records it as a new save.
///
/// Unsaved changes are handled as `policy` says; with [`UnsavedPolicy::MoveToRecovery`] the
/// recovery entry made for them is returned.
pub fn bring_back_everything(
    p: &Project,
    id: &SaveId,
    policy: UnsavedPolicy,
) -> Result<Option<RecoveryEntry>, BringBackError> {
    let has_changes = !status(p.repo())
        .map_err(CoreError::from)?
        .entries
        .is_empty();
    let set_aside = match (has_changes, policy) {
        (true, UnsavedPolicy::RequireClean) => return Err(BringBackError::HasUnsavedChanges),
        (true, UnsavedPolicy::MoveToRecovery) => throw_away(p, &Scope::All)?,
        (false, _) => None,
    };
    restore_worktree(p.repo(), &id.0, ":/").map_err(CoreError::from)?;
    Ok(set_aside)
}

/// True when git reports `path` as changed, including as the old name of a rename.
fn has_unsaved_changes_in(p: &Project, path: &str) -> Result<bool, CoreError> {
    Ok(status(p.repo())?
        .entries
        .iter()
        .any(|entry| entry.path == path || entry.orig_path.as_deref() == Some(path)))
}
