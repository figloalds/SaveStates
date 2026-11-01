//! Bringing the past back: a file, or the whole project, copied from an old save into the project
//! folder as unsaved changes. Saves, and the timeline, stay as they are.

use std::fs;
use std::io;

use savestates_git::{blob_size, first_parent, ls_tree_names, restore_worktree, status};

use crate::error::CoreError;
use crate::model::SaveId;
use crate::project::Project;
use crate::recovery::{
    RecoveryEntry, Scope, fold, ignores_case, set_aside_ignored_colliding, set_aside_ignored_named,
    throw_away, throw_away_paths,
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
/// What is in the folder that this would overwrite or remove is set aside first, and the recovery
/// entry is returned so the caller can offer to undo. Unsaved changes, new files included, are
/// thrown away as one entry: the one at that path, and when the file is written, a file where it
/// needs a folder and the files in a folder where it needs to be a file. Ignored files (which git
/// does not list as changes) are snapshotted in an entry of their own, by the same rule. A path
/// spelled in another case than git reports it is found on a case-insensitive project. The most
/// recent entry made is returned; when there are two, the one for the unsaved changes.
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

    // Ignored first, so that the entry for the unsaved changes is the more recent one.
    let writes = source_with_file.is_some();
    let ignored_set_aside = if writes {
        // Writing the file needs its path, and a folder in place of each folder above it.
        set_aside_ignored_colliding(p, &[path.to_owned()])?
    } else {
        // Deleting it takes nothing but the file of that name.
        set_aside_ignored_named(p, path)?
    };
    let changes_set_aside = throw_away_paths(p, &unsaved_in_the_way(p, path, writes)?)?;

    match source_with_file {
        // Literal and from the project's top, whatever the name holds (`[id].js`, accents).
        Some(rev) => restore_worktree(p.repo(), &rev, &format!(":(top,literal){path}"))?,
        None => match fs::remove_file(p.root().join(path)) {
            Ok(()) => {}
            // Nothing there, or a file where the path needs a folder: either way, nothing to delete.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) => {}
            Err(e) => return Err(e.into()),
        },
    }
    Ok(changes_set_aside.or(ignored_set_aside))
}

/// Makes the project folder match the save `id`: files are restored, added back and removed.
/// The result shows up as unsaved changes; a Quick Save then records it as a new save.
///
/// Unsaved changes are handled as `policy` says. Ignored files are left alone, except those the
/// save would overwrite or remove: git does not list them as changes, so they are snapshotted
/// whatever the policy. That is an ignored file at a path the save has a file for, an ignored
/// file where the save needs a folder, and the ignored files in a folder where the save has a
/// file. Never the rest of what is ignored, such as `node_modules`.
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
    let ignored_set_aside = set_aside_ignored_colliding(p, &in_save)?;
    let changes_set_aside = if has_changes {
        throw_away(p, &Scope::All)?
    } else {
        None
    };
    restore_worktree(p.repo(), &id.0, ":/").map_err(CoreError::from)?;
    Ok(changes_set_aside.or(ignored_set_aside))
}

/// The unsaved changes (including new files) that putting a file at `path` would overwrite or
/// remove, as the paths git lists them by, ready for [`throw_away_paths`]: the change at `path`
/// itself; and, when the file is written (`writes`), a change that is a file where `path` needs
/// a folder, and the changes inside a folder where `path` needs a file (as that folder, one path
/// for all of them). A rename counts under either of its names. On a case-insensitive project
/// the comparison ignores letter case, and the paths are spelled as git has them.
fn unsaved_in_the_way(p: &Project, path: &str, writes: bool) -> Result<Vec<String>, CoreError> {
    let fold_case = ignores_case(p)?;
    let wanted = fold(path, fold_case);
    let depth = path.split('/').count();
    let mut in_the_way: Vec<String> = Vec::new();
    for entry in status(p.repo())?.entries {
        for name in std::iter::once(entry.path).chain(entry.orig_path) {
            let key = fold(&name, fold_case);
            // The path itself, or (when writing) a file where a folder is needed.
            let spelled = if key == wanted || (writes && wanted.starts_with(&format!("{key}/"))) {
                Some(name)
            } else if writes && key.starts_with(&format!("{wanted}/")) {
                // Inside the folder: the folder, as git spells it, stands for all of them.
                Some(name.split('/').take(depth).collect::<Vec<_>>().join("/"))
            } else {
                None
            };
            if let Some(spelled) = spelled.filter(|spelled| !in_the_way.contains(spelled)) {
                in_the_way.push(spelled);
            }
        }
    }
    Ok(in_the_way)
}
