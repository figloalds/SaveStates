//! Throwing changes away without losing them: each throw-away is first snapshotted on a private
//! ref, from which the Recovery list can bring it back for 30 days.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use savestates_git::{
    GitError, RunOpts, config_get, delete_ref, ls_tree_names, refs_with_prefix, stash_apply,
    stash_snapshot_files_to_ref, stash_snapshot_to_ref, status,
};

use crate::error::CoreError;
use crate::project::Project;

/// Where recovery snapshots live. A ref is named `<unix seconds>-<n>` under this prefix.
pub const RECOVERY_PREFIX: &str = "refs/savestates/recovery/";

/// Recovery entries older than this are removed by [`prune_recovery`].
pub const RECOVERY_MAX_AGE_DAYS: i64 = 30;

const SECONDS_PER_DAY: i64 = 86_400;

/// Work that was thrown away and can be brought back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryEntry {
    /// The full name of the ref holding the snapshot; pass it to [`restore_recovery`].
    pub ref_name: String,
    /// When the work was thrown away, in seconds since the Unix epoch.
    pub created_unix: i64,
    /// The files in the snapshot, sorted: the changed ones and the new ones (for a snapshot of
    /// ignored files, those).
    pub files: Vec<String>,
}

/// What to throw away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    /// One file (or folder) by its path relative to the project folder, taken literally.
    File(String),
}

/// Snapshots the unsaved changes in `scope`, then removes them from the project folder. Ignored
/// files are neither snapshotted nor touched: they are not changes. `None` when there was
/// nothing to throw away.
///
/// Fails with [`CoreError::NoSavesYet`] before the first save, even when there are changes:
/// there is no save to go back to.
///
/// If git fails part-way, after it set the work aside but before it finished removing it from
/// the folder (a file in use, say), the error is returned, and the work is in the Recovery list
/// all the same: [`list_recovery`] has it.
pub fn throw_away(p: &Project, scope: &Scope) -> Result<Option<RecoveryEntry>, CoreError> {
    if status(p.repo())?.head_oid.is_none() {
        return Err(CoreError::NoSavesYet);
    }
    let path = match scope {
        Scope::All => None,
        Scope::File(path) => Some(path.as_str()),
    };
    let (created_unix, ref_name) = next_ref_name(p)?;
    let Some(oid) = stash_snapshot_to_ref(p.repo(), path, &ref_name)? else {
        return Ok(None);
    };
    entry(p, ref_name, created_unix, &oid).map(Some)
}

/// Everything in the Recovery list, newest first.
pub fn list_recovery(p: &Project) -> Result<Vec<RecoveryEntry>, CoreError> {
    let mut found: Vec<_> = refs_with_prefix(p.repo(), RECOVERY_PREFIX)?
        .into_iter()
        .filter_map(|(ref_name, oid)| {
            let (created_unix, n) = parse_ref_name(&ref_name)?;
            Some((created_unix, n, ref_name, oid))
        })
        .collect();
    found.sort_by_key(|(created_unix, n, ..)| Reverse((*created_unix, *n)));
    found
        .into_iter()
        .map(|(created_unix, _, ref_name, oid)| entry(p, ref_name, created_unix, &oid))
        .collect()
}

/// Puts the thrown-away work in `ref_name` back in the project folder as unsaved changes, and
/// removes it from the Recovery list.
///
/// What is in the folder that this would overwrite is set aside first, as entries of its own
/// (so it stays recoverable):
/// - unsaved changes are thrown away;
/// - ignored files on disk at paths the entry holds are snapshotted too, because git does not
///   list them as changes and would write over them. No other ignored file is touched.
///
/// The entry returned is the most recent one made for that: the one for the unsaved changes if
/// there were any, else the one for the ignored files. The other is in [`list_recovery`].
///
/// If the snapshot cannot be applied cleanly, for instance because the same lines were saved
/// differently since, the folder is returned to the latest save, the entry stays in the list,
/// and the result is [`CoreError::RecoveryConflict`]. Whatever was set aside first is in the
/// list too.
pub fn restore_recovery(p: &Project, ref_name: &str) -> Result<Option<RecoveryEntry>, CoreError> {
    let oid = refs_with_prefix(p.repo(), RECOVERY_PREFIX)?
        .into_iter()
        .find_map(|(name, oid)| (name == ref_name).then_some(oid))
        .ok_or_else(|| CoreError::NotFound(ref_name.to_owned()))?;

    // Ignored files first, so that the entry for the unsaved changes is the more recent one.
    let ignored_set_aside = set_aside_ignored_among(p, &files_in_snapshot(p, &oid)?)?;
    let changes_set_aside = if status(p.repo())?.entries.is_empty() {
        None
    } else {
        throw_away(p, &Scope::All)?
    };
    let set_aside = changes_set_aside.or(ignored_set_aside);
    // Applying onto changes that could not be set aside would mix them up with the snapshot.
    if !status(p.repo())?.entries.is_empty() {
        return Err(CoreError::RecoveryConflict);
    }

    match stash_apply(p.repo(), &oid) {
        Ok(()) => {}
        Err(GitError::Failed { .. }) => {
            abandon_apply(p)?;
            return Err(CoreError::RecoveryConflict);
        }
        Err(e) => return Err(e.into()),
    }
    delete_ref(p.repo(), ref_name)?;
    Ok(set_aside)
}

/// Removes the entries older than [`RECOVERY_MAX_AGE_DAYS`] as of `now_unix` and returns how
/// many it removed. Refs under the recovery prefix that are not named like an entry are left alone.
pub fn prune_recovery(p: &Project, now_unix: i64) -> Result<usize, CoreError> {
    let cutoff = now_unix.saturating_sub(RECOVERY_MAX_AGE_DAYS * SECONDS_PER_DAY);
    let mut pruned = 0;
    for (ref_name, _) in refs_with_prefix(p.repo(), RECOVERY_PREFIX)? {
        if parse_ref_name(&ref_name).is_some_and(|(created_unix, _)| created_unix < cutoff) {
            delete_ref(p.repo(), &ref_name)?;
            pruned += 1;
        }
    }
    Ok(pruned)
}

/// A ref name no entry uses yet, with the time it carries.
fn next_ref_name(p: &Project) -> Result<(i64, String), CoreError> {
    let taken: HashSet<String> = refs_with_prefix(p.repo(), RECOVERY_PREFIX)?
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let now = now_unix();
    let mut n = 0u32;
    loop {
        let name = format!("{RECOVERY_PREFIX}{now}-{n}");
        if !taken.contains(&name) {
            return Ok((now, name));
        }
        n += 1;
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

/// The time and counter in a ref named `<RECOVERY_PREFIX><unix seconds>-<n>`.
fn parse_ref_name(ref_name: &str) -> Option<(i64, u32)> {
    let (created_unix, n) = ref_name.strip_prefix(RECOVERY_PREFIX)?.split_once('-')?;
    Some((created_unix.parse().ok()?, n.parse().ok()?))
}

fn entry(
    p: &Project,
    ref_name: String,
    created_unix: i64,
    oid: &str,
) -> Result<RecoveryEntry, CoreError> {
    Ok(RecoveryEntry {
        ref_name,
        created_unix,
        files: files_in_snapshot(p, oid)?,
    })
}

/// The files a stash-shaped commit holds: those changed from its first parent (the latest save
/// when it was made) and, when it has a third parent, the untracked files stored there. A
/// commit with no parent holds no files.
fn files_in_snapshot(p: &Project, oid: &str) -> Result<Vec<String>, CoreError> {
    let line = p.repo().run(
        &["rev-list", "--parents", "-n", "1", oid, "--"],
        &RunOpts::read_only(),
    )?;
    // The line is `<oid> <parent> <parent> <parent>`.
    let line = String::from_utf8_lossy(&line);
    let mut parents = line.split_whitespace().skip(1);
    let (base, untracked) = (parents.next(), parents.nth(1));

    let mut files = Vec::new();
    if let Some(base) = base {
        let changed = p.repo().run(
            &["diff", "--name-only", "-z", "--no-renames", base, oid, "--"],
            &RunOpts::read_only(),
        )?;
        files.extend(
            changed
                .split(|&b| b == 0)
                .filter(|name| !name.is_empty())
                .map(|name| String::from_utf8_lossy(name).into_owned()),
        );
    }
    if let Some(untracked) = untracked {
        files.extend(ls_tree_names(p.repo(), untracked)?);
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Puts the tracked files and the index back to the latest save after a `stash apply` that
/// stopped half-way. New files the apply wrote are left: they are the user's own work.
///
/// Only called on a folder that had no unsaved changes before the apply, so nothing but what
/// the apply wrote is lost.
fn abandon_apply(p: &Project) -> Result<(), CoreError> {
    // The index first (it moves nothing else, HEAD stays): `restore` refuses a path that is
    // unmerged, as one deleted by a save and edited by the snapshot is.
    p.repo()
        .run(&["reset", "-q", "--", ":/"], &RunOpts::default())?;
    p.repo().run(
        &["restore", "--source=HEAD", "--worktree", "--", ":/"],
        &RunOpts::default(),
    )?;
    Ok(())
}

/// Snapshots the ignored files on disk that are also in `among`, and removes them from the
/// folder, so that something about to be written there cannot overwrite them. Git does not list
/// ignored files as changes, so the usual throw-away does not see them. Only those files, never
/// the rest of what is ignored (`node_modules`). `None` when there are none.
pub(crate) fn set_aside_ignored_among(
    p: &Project,
    among: &[String],
) -> Result<Option<RecoveryEntry>, CoreError> {
    let fold_case = ignores_case(p)?;
    let key = |name: &str| {
        if fold_case {
            name.to_lowercase()
        } else {
            name.to_owned()
        }
    };
    let wanted: HashSet<String> = among.iter().map(|name| key(name)).collect();
    let files = ignored_files_on_disk(p, None)?
        .into_iter()
        .filter(|name| wanted.contains(&key(name)))
        .collect();
    set_aside_files(p, files)
}

/// [`set_aside_ignored_among`] for the one file at `path`, if it is ignored and on disk.
pub(crate) fn set_aside_ignored_file(
    p: &Project,
    path: &str,
) -> Result<Option<RecoveryEntry>, CoreError> {
    set_aside_files(p, ignored_files_on_disk(p, Some(path))?)
}

fn set_aside_files(p: &Project, files: Vec<String>) -> Result<Option<RecoveryEntry>, CoreError> {
    if files.is_empty() {
        return Ok(None);
    }
    let (created_unix, ref_name) = next_ref_name(p)?;
    let Some(oid) = stash_snapshot_files_to_ref(p.repo(), &files, &ref_name)? else {
        return Ok(None);
    };
    entry(p, ref_name, created_unix, &oid).map(Some)
}

/// The files on disk that git ignores and does not track, with their paths. All of them, or with
/// `only`, those at that path (taken literally).
fn ignored_files_on_disk(p: &Project, only: Option<&str>) -> Result<Vec<String>, CoreError> {
    let mut args = Vec::new();
    if only.is_some() {
        args.push("--literal-pathspecs");
    }
    args.extend([
        "ls-files",
        "-z",
        "--others",
        "--ignored",
        "--exclude-standard",
    ]);
    if let Some(path) = only {
        args.extend(["--", path]);
    }
    let out = p.repo().run(&args, &RunOpts::read_only())?;
    Ok(out
        .split(|&b| b == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect())
}

/// True when this project treats `readme.md` and `README.md` as one file (`core.ignorecase`,
/// which git sets from what the file system does).
pub(crate) fn ignores_case(p: &Project) -> Result<bool, CoreError> {
    Ok(
        config_get(p.repo(), "core.ignorecase")?.is_some_and(|value| {
            ["true", "yes", "on", "1"]
                .iter()
                .any(|truthy| value.trim().eq_ignore_ascii_case(truthy))
        }),
    )
}
