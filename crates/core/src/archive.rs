//! Renaming, archiving and deleting timelines. A deleted timeline keeps a private copy of its tip
//! and names for 30 days, from which [`restore_deleted`] brings it back.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use savestates_git::{
    GitError, config_get_regexp, config_remove_section, config_set, config_unset, create_branch,
    delete_branch, delete_ref, list_branches, log_first_parent, own_first_parent_saves,
    refs_with_prefix, update_ref,
};

use crate::error::CoreError;
use crate::project::Project;
use crate::timeline::{SaveState, into_save_state};
use crate::timelines::{Timeline, list_timelines};

/// Where deleted timelines are kept. A ref is named `<unix seconds>-<n>` under this prefix.
pub const DELETED_PREFIX: &str = "refs/savestates/deleted/";

const DELETED_SECTION: &str = "savestates-deleted";

#[derive(thiserror::Error, Debug)]
pub enum ArchiveError {
    #[error("the main line cannot be archived or deleted")]
    MainLine,
    #[error("the timeline the folder is on cannot be archived or deleted")]
    Current,
    #[error("the timeline is open in another folder: {0}")]
    Elsewhere(PathBuf),
    #[error("no such timeline")]
    NotFound,
    #[error(transparent)]
    Core(#[from] CoreError),
}

impl From<GitError> for ArchiveError {
    fn from(e: GitError) -> Self {
        Self::Core(e.into())
    }
}

/// A timeline set aside, with the numbers the archive list shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchivedTimeline {
    pub branch: String,
    pub display_name: Option<String>,
    /// The saves only this timeline has.
    pub own_saves: usize,
    pub last_save: SaveState,
}

/// A deleted timeline that can still be brought back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeletedTimeline {
    /// The full name of the ref holding its tip; pass it to [`restore_deleted`].
    pub ref_name: String,
    pub branch: String,
    pub display_name: Option<String>,
    pub deleted_unix: i64,
}

fn description_key(branch: &str) -> String {
    format!("branch.{branch}.description")
}

fn archived_key(branch: &str) -> String {
    format!("branch.{branch}.savestates-archived")
}

/// Gives the timeline a display name; an empty (or blank) name goes back to the default. The git
/// branch keeps its name.
pub fn rename_timeline(p: &Project, branch: &str, name: &str) -> Result<(), ArchiveError> {
    find(p, branch)?;
    let name = name.trim();
    if name.is_empty() {
        config_unset(p.repo(), &description_key(branch))?;
    } else {
        config_set(p.repo(), &description_key(branch), name)?;
    }
    Ok(())
}

/// Hides the timeline from the main view. Refuses the main line, the current timeline and one
/// open in another folder.
pub fn archive_timeline(p: &Project, branch: &str) -> Result<(), ArchiveError> {
    removable(p, branch)?;
    config_set(p.repo(), &archived_key(branch), "true")?;
    Ok(())
}

pub fn unarchive_timeline(p: &Project, branch: &str) -> Result<(), ArchiveError> {
    find(p, branch)?;
    config_unset(p.repo(), &archived_key(branch))?;
    Ok(())
}

/// The archived timelines, the one saved to most recently first.
pub fn list_archived(p: &Project) -> Result<Vec<ArchivedTimeline>, CoreError> {
    let branches = list_branches(p.repo())?;
    let names: HashMap<String, Option<String>> = list_timelines(p)?
        .into_iter()
        .map(|t| (t.branch, t.display_name))
        .collect();
    let mut archived = Vec::new();
    for b in branches.iter().filter(|b| b.archived) {
        let others: Vec<String> = branches
            .iter()
            .filter(|o| o.name != b.name)
            .map(|o| o.tip.clone())
            .collect();
        let own_saves = own_first_parent_saves(p.repo(), &b.tip, &others)?.len();
        let Some(entry) = log_first_parent(p.repo(), &b.tip, 0, 1)?.into_iter().next() else {
            continue;
        };
        archived.push(ArchivedTimeline {
            branch: b.name.clone(),
            display_name: names.get(&b.name).cloned().flatten(),
            own_saves,
            last_save: into_save_state(entry),
        });
    }
    archived.sort_by_key(|a| Reverse(a.last_save.committed));
    Ok(archived)
}

/// Deletes the timeline, keeping its tip and names under [`DELETED_PREFIX`] so
/// [`restore_deleted`] can bring it back. Refuses like [`archive_timeline`].
pub fn delete_timeline(p: &Project, branch: &str) -> Result<DeletedTimeline, ArchiveError> {
    let timeline = removable(p, branch)?;
    let taken: HashSet<String> = refs_with_prefix(p.repo(), DELETED_PREFIX)?
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let now = now_unix();
    let mut n = 0u32;
    let (id, ref_name) = loop {
        let id = format!("{now}-{n}");
        let ref_name = format!("{DELETED_PREFIX}{id}");
        if !taken.contains(&ref_name) {
            break (id, ref_name);
        }
        n += 1;
    };

    update_ref(p.repo(), &ref_name, &timeline.tip.0)?;
    let kept = config_set(p.repo(), &format!("{DELETED_SECTION}.{id}.branch"), branch)
        .and_then(|()| match &timeline.display_name {
            Some(name) => config_set(p.repo(), &format!("{DELETED_SECTION}.{id}.name"), name),
            None => Ok(()),
        })
        .and_then(|()| delete_branch(p.repo(), branch));
    if let Err(e) = kept {
        // When git deleted the branch all the same, the copy is all that is left of it: kept.
        let still_there =
            list_branches(p.repo()).map_or(true, |all| all.iter().any(|b| b.name == branch));
        if still_there {
            // Git refused (another git program holding it, say): the copy goes again, so the
            // timeline is not listed as deleted too. Best effort: what stays is pruned after
            // 30 days.
            let _ = delete_ref(p.repo(), &ref_name);
            let _ = remove_deleted_section(p, &ref_name);
            return Err(e.into());
        }
    }
    Ok(DeletedTimeline {
        ref_name,
        branch: branch.to_owned(),
        display_name: timeline.display_name,
        deleted_unix: now,
    })
}

/// The deleted timelines, newest first.
pub fn list_deleted(p: &Project) -> Result<Vec<DeletedTimeline>, CoreError> {
    let settings = config_get_regexp(p.repo(), r"^savestates-deleted\.")?;
    let mut deleted = Vec::new();
    for (ref_name, _) in refs_with_prefix(p.repo(), DELETED_PREFIX)? {
        let Some((deleted_unix, n)) = parse_deleted_name(&ref_name) else {
            continue;
        };
        let setting = |variable: &str| {
            let key = format!("{DELETED_SECTION}.{deleted_unix}-{n}.{variable}");
            settings
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.trim_end_matches(['\r', '\n']).to_owned())
        };
        let Some(branch) = setting("branch") else {
            continue;
        };
        deleted.push((
            (deleted_unix, n),
            DeletedTimeline {
                ref_name,
                branch,
                display_name: setting("name"),
                deleted_unix,
            },
        ));
    }
    deleted.sort_by_key(|(key, _)| Reverse(*key));
    Ok(deleted.into_iter().map(|(_, d)| d).collect())
}

/// Brings a deleted timeline back under its old name, or the first free `<name>-2`, `<name>-3`…
/// It comes back archived, with its display name. Returns the branch name used.
pub fn restore_deleted(p: &Project, ref_name: &str) -> Result<String, CoreError> {
    let deleted = list_deleted(p)?
        .into_iter()
        .find(|d| d.ref_name == ref_name)
        .ok_or_else(|| CoreError::NotFound(ref_name.to_owned()))?;
    let oid = refs_with_prefix(p.repo(), ref_name)?
        .into_iter()
        .find_map(|(name, oid)| (name == ref_name).then_some(oid))
        .ok_or_else(|| CoreError::NotFound(ref_name.to_owned()))?;

    let existing: HashSet<String> = list_branches(p.repo())?
        .into_iter()
        .map(|b| b.name)
        .collect();
    let mut name = deleted.branch.clone();
    let mut n = 2u32;
    while existing.contains(&name) {
        name = format!("{}-{n}", deleted.branch);
        n += 1;
    }

    create_branch(p.repo(), &name, &oid)?;
    if let Some(display) = &deleted.display_name {
        config_set(p.repo(), &description_key(&name), display)?;
    }
    config_set(p.repo(), &archived_key(&name), "true")?;
    delete_ref(p.repo(), ref_name)?;
    remove_deleted_section(p, ref_name)?;
    Ok(name)
}

/// Removes the deleted timelines older than `cutoff_unix` (ref and settings); returns how many.
pub(crate) fn prune_deleted(p: &Project, cutoff_unix: i64) -> Result<usize, CoreError> {
    let mut pruned = 0;
    for (ref_name, _) in refs_with_prefix(p.repo(), DELETED_PREFIX)? {
        if parse_deleted_name(&ref_name).is_some_and(|(unix, _)| unix < cutoff_unix) {
            delete_ref(p.repo(), &ref_name)?;
            remove_deleted_section(p, &ref_name)?;
            pruned += 1;
        }
    }
    Ok(pruned)
}

fn remove_deleted_section(p: &Project, ref_name: &str) -> Result<(), CoreError> {
    if let Some(id) = ref_name.strip_prefix(DELETED_PREFIX) {
        config_remove_section(p.repo(), &format!("{DELETED_SECTION}.{id}"))?;
    }
    Ok(())
}

fn find(p: &Project, branch: &str) -> Result<Timeline, ArchiveError> {
    list_timelines(p)?
        .into_iter()
        .find(|t| t.branch == branch)
        .ok_or(ArchiveError::NotFound)
}

/// The timeline, if it may be archived or deleted.
fn removable(p: &Project, branch: &str) -> Result<Timeline, ArchiveError> {
    let t = find(p, branch)?;
    if t.main_line {
        Err(ArchiveError::MainLine)
    } else if t.current {
        Err(ArchiveError::Current)
    } else if let Some(folder) = t.elsewhere {
        Err(ArchiveError::Elsewhere(folder))
    } else {
        Ok(t)
    }
}

fn parse_deleted_name(ref_name: &str) -> Option<(i64, u32)> {
    let (unix, n) = ref_name.strip_prefix(DELETED_PREFIX)?.split_once('-')?;
    Some((unix.parse().ok()?, n.parse().ok()?))
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}
