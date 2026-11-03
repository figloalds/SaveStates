//! Throwing changes away without losing them: each throw-away is first snapshotted on a private
//! ref, from which the Recovery list can bring it back for 30 days.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use savestates_git::{
    EntryKind, GitError, RunOpts, append_info_exclude, config_get, delete_ref, ls_tree_names,
    refs_with_prefix, snapshot_files_to_ref, stash_apply, stash_snapshot_paths_to_ref,
    stash_snapshot_to_ref, status, update_ref,
};

use crate::archive::prune_deleted;
use crate::error::CoreError;
use crate::load::{KEEP_IGNORED_COMMENT, exclude_pattern, line_safe};
use crate::project::Project;

/// Where recovery snapshots live. A ref is named `<unix seconds>-<n>` under this prefix.
pub const RECOVERY_PREFIX: &str = "refs/savestates/recovery/";

/// Where Load marks the recovery entries it made for ignored files in the way (spec §4.3 step 3),
/// so they come back by themselves. A marker has the same `<unix seconds>-<n>` name as the
/// recovery ref it marks and points at the same snapshot.
pub const LOAD_ASIDE_PREFIX: &str = "refs/savestates/load-aside/";

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
    match scope {
        Scope::All => throw_away_selected(p, &[]),
        Scope::File(path) => throw_away_selected(p, std::slice::from_ref(path)),
    }
}

/// [`throw_away`] for the changes at several paths (files or folders, taken literally) as one
/// entry. [`Scope::File`] takes one path; this is for the callers that need a handful of paths
/// set aside together, and stays out of the public API. No paths throws nothing away.
pub(crate) fn throw_away_paths(
    p: &Project,
    paths: &[String],
) -> Result<Option<RecoveryEntry>, CoreError> {
    if paths.is_empty() {
        return Ok(None);
    }
    throw_away_selected(p, paths)
}

/// Throws away the changes at `paths`; everything when `paths` is empty.
fn throw_away_selected(p: &Project, paths: &[String]) -> Result<Option<RecoveryEntry>, CoreError> {
    if status(p.repo())?.head_oid.is_none() {
        return Err(CoreError::NoSavesYet);
    }
    let (created_unix, ref_name) = next_ref_name(p)?;
    let snapshot = match paths {
        [] => stash_snapshot_to_ref(p.repo(), None, &ref_name)?,
        paths => stash_snapshot_paths_to_ref(p.repo(), paths, &ref_name)?,
    };
    let Some(oid) = snapshot else {
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
/// - ignored files the entry would overwrite or remove (at its paths, or in the way of them as a
///   file where a folder is needed, or a folder where a file is needed) are snapshotted too,
///   because git does not list them as changes and would write over them. No other ignored file
///   is touched.
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
    let ignored_set_aside = set_aside_ignored_colliding(p, &files_in_snapshot(p, &oid)?)?;
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

/// Removes the entries older than [`RECOVERY_MAX_AGE_DAYS`] as of `now_unix`, the deleted
/// timelines that old, and the Load markers whose entry is gone; returns how many it removed in
/// all. An entry still marked as set aside by Load (a marker of the same name on the same
/// snapshot) is kept however old, with its marker: it waits to come back by itself. Refs under
/// these prefixes that are not named like ours are left alone.
pub fn prune_recovery(p: &Project, now_unix: i64) -> Result<usize, CoreError> {
    let cutoff = now_unix.saturating_sub(RECOVERY_MAX_AGE_DAYS * SECONDS_PER_DAY);
    let markers: HashMap<String, String> = refs_with_prefix(p.repo(), LOAD_ASIDE_PREFIX)?
        .into_iter()
        .map(|(name, oid)| (name[LOAD_ASIDE_PREFIX.len()..].to_owned(), oid))
        .collect();
    let mut pruned = 0;
    for (ref_name, oid) in refs_with_prefix(p.repo(), RECOVERY_PREFIX)? {
        let marked = markers.get(&ref_name[RECOVERY_PREFIX.len()..]) == Some(&oid);
        if !marked
            && parse_ref_name(&ref_name).is_some_and(|(created_unix, _)| created_unix < cutoff)
        {
            delete_ref(p.repo(), &ref_name)?;
            pruned += 1;
        }
    }
    let entries: HashSet<String> = refs_with_prefix(p.repo(), RECOVERY_PREFIX)?
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for (marker, _) in refs_with_prefix(p.repo(), LOAD_ASIDE_PREFIX)? {
        let entry = format!("{RECOVERY_PREFIX}{}", &marker[LOAD_ASIDE_PREFIX.len()..]);
        if parse_ref_name(&entry).is_some() && !entries.contains(&entry) {
            delete_ref(p.repo(), &marker)?;
            pruned += 1;
        }
    }
    Ok(pruned + prune_deleted(p, cutoff)?)
}

/// Drops the marker [`mark_load_aside`] put on `entry`, if there is one.
pub(crate) fn delete_load_aside(p: &Project, entry: &RecoveryEntry) -> Result<(), CoreError> {
    if let Some(suffix) = entry.ref_name.strip_prefix(RECOVERY_PREFIX) {
        let marker = format!("{LOAD_ASIDE_PREFIX}{suffix}");
        if refs_with_prefix(p.repo(), &marker)?
            .iter()
            .any(|(name, _)| *name == marker)
        {
            delete_ref(p.repo(), &marker)?;
        }
    }
    Ok(())
}

/// Marks `entry` as set aside by Load, so [`bring_back_load_aside`] restores it once its paths
/// are free.
pub(crate) fn mark_load_aside(p: &Project, entry: &RecoveryEntry) -> Result<(), CoreError> {
    let not_found = || CoreError::NotFound(entry.ref_name.clone());
    let suffix = entry
        .ref_name
        .strip_prefix(RECOVERY_PREFIX)
        .ok_or_else(not_found)?;
    let oid = refs_with_prefix(p.repo(), &entry.ref_name)?
        .into_iter()
        .find_map(|(name, oid)| (name == entry.ref_name).then_some(oid))
        .ok_or_else(not_found)?;
    update_ref(p.repo(), &format!("{LOAD_ASIDE_PREFIX}{suffix}"), &oid)?;
    Ok(())
}

/// Restores, oldest first, each entry marked by [`mark_load_aside`] whose files are all free: not
/// in the current save (nor in the way of one of its files) and absent from the folder. Each one
/// restored loses its marker; so does a marker whose entry is gone or was replaced. Returns how
/// many came back.
///
/// Call it on a folder with no unsaved changes; it stops when the folder has some, so restoring
/// never sets the folder's own changes aside. The files restored were ignored when Load set them
/// aside; any of them the current save does not ignore is added to the private exclude list, so
/// they stay out of the changes (and out of the way of the next entry).
///
/// An entry that cannot be applied cleanly stays in the Recovery list with its marker.
pub(crate) fn bring_back_load_aside(p: &Project) -> Result<usize, CoreError> {
    let mut markers: Vec<(i64, u32, String, String)> =
        refs_with_prefix(p.repo(), LOAD_ASIDE_PREFIX)?
            .into_iter()
            .filter_map(|(name, oid)| {
                let suffix = name.strip_prefix(LOAD_ASIDE_PREFIX)?;
                let (created_unix, n) = parse_ref_name(&format!("{RECOVERY_PREFIX}{suffix}"))?;
                Some((created_unix, n, name, oid))
            })
            .collect();
    if markers.is_empty() {
        return Ok(0);
    }
    markers.sort();
    let entries: HashMap<String, String> = refs_with_prefix(p.repo(), RECOVERY_PREFIX)?
        .into_iter()
        .collect();
    let fold_case = ignores_case(p)?;

    let mut brought_back = 0;
    for (_, _, marker, oid) in markers {
        let suffix = &marker[LOAD_ASIDE_PREFIX.len()..];
        let entry = format!("{RECOVERY_PREFIX}{suffix}");
        if entries.get(&entry) != Some(&oid) {
            delete_ref(p.repo(), &marker)?;
            continue;
        }
        if !status(p.repo())?.entries.is_empty() {
            break;
        }
        let files = files_in_snapshot(p, &oid)?;
        let in_save = ls_tree_names(p.repo(), "HEAD")?;
        let free = colliding(&files, &in_save, fold_case).is_empty()
            && files.iter().all(|file| absent(p.root(), file));
        if !free {
            continue;
        }
        match restore_recovery(p, &entry) {
            Ok(_) => {}
            Err(CoreError::RecoveryConflict) => continue,
            Err(e) => return Err(e),
        }
        delete_ref(p.repo(), &marker)?;
        brought_back += 1;

        let shown: Vec<String> = status(p.repo())?
            .entries
            .into_iter()
            .filter(|e| e.kind == EntryKind::Untracked && files.contains(&e.path))
            .map(|e| e.path)
            .filter(|path| line_safe(path))
            .map(|path| exclude_pattern(&path))
            .collect();
        append_info_exclude(p.repo(), KEEP_IGNORED_COMMENT, &shown)?;
    }
    Ok(brought_back)
}

/// True when nothing at all is at `relative` under `root` (not even a broken link).
pub(crate) fn absent(root: &Path, relative: &str) -> bool {
    matches!(
        fs::symlink_metadata(root.join(relative)),
        Err(e) if e.kind() == io::ErrorKind::NotFound
    )
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

/// Snapshots the ignored files on disk that writing `targets` (paths of files) would overwrite or
/// remove, then removes them from the folder. Git does not list ignored files as changes, so the
/// usual throw-away does not see them. Only those files, never the rest of what is ignored
/// (`node_modules`). `None` when there are none. See [`colliding`] for what is in the way.
pub(crate) fn set_aside_ignored_colliding(
    p: &Project,
    targets: &[String],
) -> Result<Option<RecoveryEntry>, CoreError> {
    let ignored = ignored_files_on_disk(p)?;
    set_aside_files(p, colliding(&ignored, targets, ignores_case(p)?))
}

/// Like [`set_aside_ignored_colliding`] for deleting the one file `path`, which gets in the way of
/// nothing but the file of that name.
pub(crate) fn set_aside_ignored_named(
    p: &Project,
    path: &str,
) -> Result<Option<RecoveryEntry>, CoreError> {
    let fold_case = ignores_case(p)?;
    let wanted = fold(path, fold_case);
    let named = ignored_files_on_disk(p)?
        .into_iter()
        .filter(|name| fold(name, fold_case) == wanted)
        .collect();
    set_aside_files(p, named)
}

/// Snapshots `files` (which exist on disk) under a new recovery ref, and only when that ref
/// exists removes them from the folder, with the folders that this leaves empty.
fn set_aside_files(p: &Project, files: Vec<String>) -> Result<Option<RecoveryEntry>, CoreError> {
    if files.is_empty() {
        return Ok(None);
    }
    let (created_unix, ref_name) = next_ref_name(p)?;
    let Some(oid) = snapshot_files_to_ref(p.repo(), &files, &ref_name)? else {
        return Ok(None);
    };
    let mut removed: Vec<String> = Vec::with_capacity(files.len());
    for file in &files {
        if let Err(e) = remove_with_empty_folders(p.root(), file) {
            // One could not be removed (in use): the ones already removed go back, from the
            // snapshot's files (its third parent), so the folder is as it was. Only then is
            // the snapshot dropped; if they cannot go back, it stays in the Recovery list.
            let back =
                removed.is_empty() || restore_paths_from(p, &format!("{oid}^3"), &removed).is_ok();
            if back {
                let _ = delete_ref(p.repo(), &ref_name);
            }
            return Err(e);
        }
        removed.push(file.clone());
    }
    entry(p, ref_name, created_unix, &oid).map(Some)
}

/// Writes `paths` (taken literally) in the folder as `source` (a commit) has them, leaving the
/// index alone. Every path must be in `source`.
pub(crate) fn restore_paths_from(
    p: &Project,
    source: &str,
    paths: &[String],
) -> Result<(), CoreError> {
    if paths.is_empty() {
        return Ok(());
    }
    let source = format!("--source={source}");
    p.repo().run(
        &[
            "--literal-pathspecs",
            "restore",
            &source,
            "--worktree",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ],
        &RunOpts {
            stdin: Some(&nul_list(paths)),
            ..RunOpts::default()
        },
    )?;
    Ok(())
}

/// Puts the index entries of `paths` (taken literally) back as the current save has them,
/// leaving the folder alone.
pub(crate) fn reset_paths(p: &Project, paths: &[String]) -> Result<(), CoreError> {
    if paths.is_empty() {
        return Ok(());
    }
    p.repo().run(
        &[
            "--literal-pathspecs",
            "reset",
            "-q",
            "HEAD",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ],
        &RunOpts {
            stdin: Some(&nul_list(paths)),
            ..RunOpts::default()
        },
    )?;
    Ok(())
}

fn nul_list(paths: &[String]) -> Vec<u8> {
    let mut list = Vec::new();
    for path in paths {
        list.extend_from_slice(path.as_bytes());
        list.push(0);
    }
    list
}

/// Deletes the file at `relative` (a missing one is fine), then each folder above it, up to but
/// not including `root`, that this left empty.
pub(crate) fn remove_with_empty_folders(root: &Path, relative: &str) -> Result<(), CoreError> {
    let file = root.join(relative);
    match remove_file_even_if_read_only(&file) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut folder = file.parent();
    while let Some(dir) = folder.filter(|dir| *dir != root && dir.starts_with(root)) {
        // Stops at the first folder that still has something in it.
        if fs::remove_dir(dir).is_err() {
            break;
        }
        folder = dir.parent();
    }
    Ok(())
}

fn remove_file_even_if_read_only(file: &Path) -> io::Result<()> {
    match fs::remove_file(file) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            // Windows refuses to delete a file with the read-only attribute.
            let mut permissions = fs::metadata(file)?.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            fs::set_permissions(file, permissions)?;
            fs::remove_file(file)
        }
        other => other,
    }
}

/// Every file that git ignores and does not track and that exists on disk, with its path. One
/// listing for the whole project: names are compared afterwards, in [`colliding`], because a
/// pathspec does not match across letter case.
fn ignored_files_on_disk(p: &Project) -> Result<Vec<String>, CoreError> {
    let out = p.repo().run(
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
        ],
        &RunOpts::read_only(),
    )?;
    Ok(out
        .split(|&b| b == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect())
}

/// The `ignored` files that writing the files `targets` would overwrite or remove:
/// - an ignored file at a target path;
/// - an ignored file where a target needs a folder (ignored `logs`, target `logs/a.txt`);
/// - an ignored file inside a folder where a target needs a file (ignored `build/out.js`, target
///   `build`).
///
/// With `fold_case` names are compared without regard to letter case, as a case-insensitive
/// file system does. Paths use `/`.
fn colliding(ignored: &[String], targets: &[String], fold_case: bool) -> Vec<String> {
    let target_keys: HashSet<String> = targets.iter().map(|t| fold(t, fold_case)).collect();
    let ignored_keys: HashSet<String> = ignored.iter().map(|f| fold(f, fold_case)).collect();

    let mut in_the_way: HashSet<&str> = HashSet::new();
    for target in &target_keys {
        if let Some(key) = ignored_keys.get(target) {
            in_the_way.insert(key);
        }
        for folder in folders_of(target) {
            if let Some(key) = ignored_keys.get(folder) {
                in_the_way.insert(key);
            }
        }
    }
    ignored
        .iter()
        .filter(|file| {
            let key = fold(file, fold_case);
            in_the_way.contains(key.as_str())
                || folders_of(&key).any(|folder| target_keys.contains(folder))
        })
        .cloned()
        .collect()
}

pub(crate) fn fold(name: &str, fold_case: bool) -> String {
    if fold_case {
        name.to_lowercase()
    } else {
        name.to_owned()
    }
}

/// The folders above a path, outermost first: `a/b/c` has `a` and `a/b`.
fn folders_of(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/').map(|(at, _)| &path[..at])
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

#[cfg(test)]
mod tests {
    use super::colliding;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn found(ignored: &[&str], targets: &[&str], fold_case: bool) -> Vec<String> {
        colliding(&names(ignored), &names(targets), fold_case)
    }

    #[test]
    fn a_file_at_a_target_path() {
        assert_eq!(found(&[".env", "x"], &[".env"], false), names(&[".env"]));
    }

    #[test]
    fn a_file_where_a_target_needs_a_folder() {
        assert_eq!(
            found(&["logs", "logsx", "a/logs"], &["logs/a.txt"], false),
            names(&["logs"])
        );
        assert_eq!(found(&["a/b"], &["a/b/c/d.txt"], false), names(&["a/b"]));
    }

    #[test]
    fn files_inside_a_folder_where_a_target_needs_a_file() {
        assert_eq!(
            found(
                &["build/out.js", "build/x/y.js", "buildx/z"],
                &["build"],
                false
            ),
            names(&["build/out.js", "build/x/y.js"])
        );
    }

    #[test]
    fn nothing_in_the_way() {
        assert!(
            found(
                &["node_modules/p/i.js", "local.log"],
                &["a.txt", "src/b.txt"],
                false
            )
            .is_empty()
        );
        assert!(found(&["x"], &[], false).is_empty());
    }

    #[test]
    fn letter_case_only_matters_when_asked() {
        assert!(found(&[".ENV"], &[".env"], false).is_empty());
        assert_eq!(found(&[".ENV"], &[".env"], true), names(&[".ENV"]));
        assert_eq!(found(&["LOGS"], &["logs/a.txt"], true), names(&["LOGS"]));
        assert_eq!(
            found(&["Build/Out.js"], &["build"], true),
            names(&["Build/Out.js"])
        );
    }
}
