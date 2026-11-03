//! Load: moving the project to any save (spec §4). Continuing the timeline that ends there, or
//! starting an app timeline at it, with the ignored files in the way kept safe.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use savestates_git::{
    Blocker, EntryKind, GitError, RunOpts, append_info_exclude, delete_branch,
    ignored_entries_by_rule, list_branches, ls_tree_names, on_first_parent_line, restore_worktree,
    status, switch_new, switch_to,
};

use crate::error::CoreError;
use crate::model::{RepoCondition, SaveId};
use crate::names::new_app_branch_name;
use crate::project::{Project, snapshot};
use crate::recovery::{
    RecoveryEntry, absent, bring_back_load_aside, delete_load_aside, fold, ignores_case,
    mark_load_aside, remove_with_empty_folders, reset_paths, restore_paths_from, restore_recovery,
    set_aside_ignored_colliding,
};
use crate::timelines::{APP_PREFIX, Timeline, list_timelines};

/// The comment written above the paths Load keeps ignored in `.git/info/exclude` (spec §4.3).
pub const KEEP_IGNORED_COMMENT: &str = "# SaveStates: kept ignored after loading an older save";

/// The branch whose name, if it exists, keeps git from creating any `savestates/…` branch.
const PREFIX_BRANCH: &str = "savestates";

/// Where a Load goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    /// Continue this timeline: its last save is the target.
    Switch(String),
    /// Start a new app timeline at the target.
    Start,
}

/// Spec §4.3 step 2: the visible timeline other than the current one whose last save is
/// `target` and that no other worktree has checked out. When several qualify: `prefer` (the
/// lane the save is drawn in, or where Desfazer returns to), then the main line, then app-made
/// timelines, then by name. None gives [`Destination::Start`].
pub fn load_destination(
    timelines: &[Timeline],
    target: &SaveId,
    prefer: Option<&str>,
) -> Destination {
    timelines
        .iter()
        .filter(|t| !t.archived && !t.current && t.elsewhere.is_none() && t.tip == *target)
        .min_by_key(|t| {
            (
                prefer != Some(t.branch.as_str()),
                !t.main_line,
                !t.app_made(),
                t.branch.as_str(),
            )
        })
        .map_or(Destination::Start, |t| {
            Destination::Switch(t.branch.clone())
        })
}

/// An anchored, literal gitignore pattern for `entry` (a path from the root; a folder ends in
/// `/`): `/` first, a backslash before `\ * ? [` and before a leading `#` or `!`, and each
/// trailing space written as `\ `.
pub fn exclude_pattern(entry: &str) -> String {
    let body = entry.trim_end_matches(' ');
    let mut pattern = String::with_capacity(entry.len() + 4);
    pattern.push('/');
    for (at, c) in body.chars().enumerate() {
        if matches!(c, '\\' | '*' | '?' | '[') || (at == 0 && matches!(c, '#' | '!')) {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    for _ in body.len()..entry.len() {
        pattern.push_str("\\ ");
    }
    pattern
}

/// Whether `path` fits on one line of an ignore file. A name with a line break cannot be
/// written there: its second line would be a pattern of its own.
pub(crate) fn line_safe(path: &str) -> bool {
    !path.contains(['\n', '\r'])
}

/// The ignored entries in `before` (files, and folders ending in `/`) that hold some path of
/// `after_untracked`: the ones the switch un-ignored. Sorted, without duplicates.
pub fn kept_ignored(before: &[String], after_untracked: &[String]) -> Vec<String> {
    let before: HashSet<&str> = before.iter().map(String::as_str).collect();
    let mut kept: Vec<String> = Vec::new();
    for path in after_untracked {
        if let Some(entry) = before.get(path.as_str()) {
            kept.push((*entry).to_owned());
        }
        // Each folder above the path, with its trailing `/`: `a/b/c` has `a/` and `a/b/`.
        for (at, _) in path.match_indices('/') {
            if let Some(entry) = before.get(&path[..=at]) {
                kept.push((*entry).to_owned());
            }
        }
    }
    kept.sort();
    kept.dedup();
    kept
}

/// Where the project was before a Load: what Desfazer loads back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadOrigin {
    pub save: SaveId,
    /// The timeline it was on; `None` on a bare save.
    pub branch: Option<String>,
}

/// What a Load did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadOutcome {
    pub origin: LoadOrigin,
    /// The branch now current.
    pub branch: String,
    /// Whether a new app timeline was started.
    pub started: bool,
    /// The ignored files that were in the way, now in the Recovery list. They come back by
    /// themselves once their paths are free.
    pub set_aside: Option<RecoveryEntry>,
    /// How many earlier set-aside snapshots came back.
    pub brought_back: usize,
    /// The patterns added to the private exclude list, so what was ignored stays ignored.
    pub kept_ignored: Vec<String>,
    /// Files git could not update (in use by another program); [`finish_load`] finishes them.
    pub left_over: Vec<String>,
    /// The empty app timeline just left, now deleted.
    pub removed: Option<String>,
    /// What went wrong after the move itself succeeded (a failing `post-checkout` hook, say).
    pub warning: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum LoadError {
    #[error("the folder has unsaved changes")]
    HasUnsavedChanges,
    #[error("the project is not in a state that can be loaded from")]
    NotNormal,
    #[error("a branch named `savestates` keeps new timelines from being made")]
    PrefixTaken,
    #[error("git could not move the project: {details}")]
    SwitchFailed {
        details: String,
        /// Files that differ from the save the project is on and that Load left alone, because
        /// they hold neither that save's content nor the target's: someone changed them while
        /// Load ran. They are unsaved changes now.
        untouched: Vec<String>,
    },
    #[error(transparent)]
    Core(#[from] CoreError),
}

impl From<GitError> for LoadError {
    fn from(e: GitError) -> Self {
        Self::Core(e.into())
    }
}

/// Moves the project to `target` (spec §4.3). Refused with unsaved changes. Continues the
/// timeline that ends at `target` (see [`load_destination`]), or starts a new app timeline
/// there. Nothing in the folder is lost: it had no unsaved changes, and the ignored files the
/// move would overwrite or remove are set aside in the Recovery list first, to come back by
/// themselves.
///
/// When git refuses without moving, the folder is put back as it was and the result is
/// [`LoadError::SwitchFailed`]. When git moved but complained (a hook), the Load stands, with
/// a `warning`. Files git could not update are in `left_over`.
pub fn load(p: &Project, target: &SaveId, prefer: Option<&str>) -> Result<LoadOutcome, LoadError> {
    load_with(p, target, prefer, || {})
}

/// [`load`], running `just_before_switch` right before git is asked to move: a seam for tests
/// that need the folder to change while Load runs.
pub(crate) fn load_with(
    p: &Project,
    target: &SaveId,
    prefer: Option<&str>,
    just_before_switch: impl FnOnce(),
) -> Result<LoadOutcome, LoadError> {
    // 1. Re-check, right before acting.
    if snapshot(p).condition != RepoCondition::Normal {
        return Err(LoadError::NotNormal);
    }
    let st = status(p.repo())?;
    if !st.entries.is_empty() {
        return Err(LoadError::HasUnsavedChanges);
    }
    let Some(head) = st.head_oid else {
        return Err(CoreError::NoSavesYet.into());
    };

    // 2. Where to.
    let timelines = list_timelines(p)?;
    let destination = load_destination(&timelines, target, prefer);
    if destination == Destination::Start && prefix_taken(p, &timelines)? {
        return Err(LoadError::PrefixTaken);
    }

    // 3. Where from.
    let origin = LoadOrigin {
        save: SaveId(head),
        branch: st.branch,
    };

    // 4. Ignored files in the way.
    let target_files = ls_tree_names(p.repo(), &target.0)?;
    let set_aside = set_aside_ignored_colliding(p, &target_files)?;

    // 5–6. What is ignored now, then the switch. Until git is asked to move, a failure puts the
    // files set aside back.
    let prepared = prepare(p, &timelines, &destination, set_aside.as_ref());
    let (before, branch, started) = match prepared {
        Ok(prepared) => prepared,
        Err(e) => {
            if let Some(entry) = &set_aside {
                put_set_aside_back(p, entry);
            }
            return Err(e);
        }
    };
    just_before_switch();
    let switched = if started {
        switch_new(p.repo(), &branch, &target.0)
    } else {
        switch_to(p.repo(), &branch)
    };

    let mut warnings = Vec::new();
    if let Err(e) = switched {
        let details = match e {
            GitError::Failed { stderr, .. } => stderr.trim().to_owned(),
            other => other.to_string(),
        };
        let moved = match head_now(p) {
            Ok((oid, branch)) => {
                oid.as_deref() != Some(origin.save.0.as_str()) || branch != origin.branch
            }
            // Nothing more is done in a folder git cannot read.
            Err(_) => {
                return Err(LoadError::SwitchFailed {
                    details,
                    untouched: Vec::new(),
                });
            }
        };
        if !moved {
            let untouched = undo_switch(p, &target.0, started.then_some(&branch));
            return Err(LoadError::SwitchFailed { details, untouched });
        }
        warnings.push(details);
    }

    // 7. What was ignored stays ignored.
    let kept_ignored = keep_ignored(p, &before).unwrap_or_else(|e| {
        warnings.push(e.to_string());
        Vec::new()
    });

    // 8. Files git could not update.
    let left_over = match status(p.repo()) {
        Ok(now) => Some(now.entries.into_iter().map(|e| e.path).collect::<Vec<_>>()),
        Err(e) => {
            warnings.push(e.to_string());
            None
        }
    };

    // 9. Set-aside files whose paths are free come back.
    let mut brought_back = 0;
    if left_over.as_ref().is_some_and(Vec::is_empty) {
        match bring_back_load_aside(p) {
            Ok(n) => brought_back = n,
            Err(e) => warnings.push(e.to_string()),
        }
    }

    // 10. Tidy: the empty app timeline just left goes.
    let removed = tidy_left(p, &origin, &branch, started).unwrap_or_else(|e| {
        warnings.push(e.to_string());
        None
    });

    Ok(LoadOutcome {
        origin,
        branch,
        started,
        set_aside,
        brought_back,
        kept_ignored,
        left_over: left_over.unwrap_or_default(),
        removed,
        warning: (!warnings.is_empty()).then(|| warnings.join("\n")),
    })
}

/// Steps 4–6 up to the switch: marks the set-aside entry, lists what is ignored now, and picks
/// the branch to move to. Returns `(ignored entries, branch, started)`.
fn prepare(
    p: &Project,
    timelines: &[Timeline],
    destination: &Destination,
    set_aside: Option<&RecoveryEntry>,
) -> Result<(Vec<String>, String, bool), LoadError> {
    if let Some(entry) = set_aside {
        mark_load_aside(p, entry)?;
    }
    let before = ignored_entries_by_rule(p.repo())?;
    Ok(match destination {
        Destination::Switch(branch) => (before, branch.clone(), false),
        Destination::Start => {
            let taken: Vec<String> = timelines.iter().map(|t| t.branch.clone()).collect();
            (before, new_app_branch_name(&taken, clock_seed()), true)
        }
    })
}

/// A seed from the clock's nanoseconds.
fn clock_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos() as u64)
}

/// Before git was asked to move: brings the files just set aside back (their paths are free:
/// nothing moved) and drops their marker. Best effort: what fails stays in the Recovery list.
fn put_set_aside_back(p: &Project, entry: &RecoveryEntry) {
    if restore_recovery(p, &entry.ref_name).is_ok() {
        let _ = delete_load_aside(p, entry);
    }
}

/// Where HEAD is: its save and its branch (`None` on a bare save or before the first save).
fn head_now(p: &Project) -> Result<(Option<String>, Option<String>), CoreError> {
    let out = match p.repo().run(
        &["rev-parse", "HEAD", "--symbolic-full-name", "HEAD"],
        &RunOpts::read_only(),
    ) {
        Ok(out) => out,
        // No save yet: HEAD names a branch with nothing on it.
        Err(GitError::Failed { .. }) => {
            let out = p
                .repo()
                .run(&["symbolic-ref", "-q", "HEAD"], &RunOpts::read_only())?;
            let name = String::from_utf8_lossy(&out).trim().to_owned();
            return Ok((None, name.strip_prefix("refs/heads/").map(str::to_owned)));
        }
        Err(e) => return Err(e.into()),
    };
    let printed = String::from_utf8_lossy(&out);
    let mut lines = printed.lines().map(str::trim);
    let oid = lines.next().filter(|l| !l.is_empty()).map(str::to_owned);
    let branch = lines
        .next()
        .and_then(|name| name.strip_prefix("refs/heads/"))
        .map(str::to_owned);
    Ok((oid, branch))
}

/// Whether a branch named `savestates` exists (in any letter case, where the file system does
/// not tell them apart), which keeps git from creating `savestates/…` branches.
fn prefix_taken(p: &Project, timelines: &[Timeline]) -> Result<bool, CoreError> {
    let fold_case = ignores_case(p)?;
    Ok(timelines
        .iter()
        .any(|t| fold(&t.branch, fold_case) == PREFIX_BRANCH))
}

/// After a switch that failed without moving HEAD, puts back only what git may have written:
/// the paths that differ between the save the project is on and `target`. For each one:
/// - the target's file, exactly as the target saved it, is written back as the current save
///   has it, or deleted when the current save has no such file;
/// - a missing file the current save has is written back;
/// - anything else is left alone and returned: someone changed it while Load ran, and it is
///   the only copy of that work.
///
/// Then deletes the app branch the switch may have created, and, when the folder is clean,
/// brings back what was set aside. Best effort: what fails stays as it is.
fn undo_switch(p: &Project, target: &str, created: Option<&String>) -> Vec<String> {
    let mut untouched = Vec::new();
    if let Ok(changed) = changed_between(p, "HEAD", target)
        && let (Ok(head), Ok(theirs)) = (tree_oids(p, "HEAD"), tree_oids(p, target))
    {
        // Git writes the files and the index before it moves HEAD: the index first.
        let _ = reset_paths(p, &changed);
        let paths: Vec<&str> = changed.iter().map(String::as_str).collect();
        let on_disk = worktree_oids(p, &paths);
        let as_target = |path: &str| {
            on_disk
                .get(path)
                .is_some_and(|oid| theirs.get(path) == Some(oid))
        };

        // Files only the target has go first, so a folder they made can become a file again.
        for path in paths.iter().filter(|path| !head.contains_key(**path)) {
            if as_target(path) {
                let _ = remove_with_empty_folders(p.root(), path);
            } else if !absent(p.root(), path) {
                untouched.push((*path).to_owned());
            }
        }
        let mut put_back = Vec::new();
        for path in paths.iter().filter(|path| head.contains_key(**path)) {
            if on_disk.get(*path) == head.get(*path) {
                continue;
            }
            if as_target(path) || absent(p.root(), path) {
                put_back.push((*path).to_owned());
            } else {
                untouched.push((*path).to_owned());
            }
        }
        if restore_paths_from(p, "HEAD", &put_back).is_err() {
            // One at a time, so a file in use does not keep the others from going back.
            for path in &put_back {
                let _ = restore_worktree(p.repo(), "HEAD", &literal(path));
            }
        }
    }
    if let Some(branch) = created {
        let not_checked_out = list_branches(p.repo()).is_ok_and(|all| {
            all.iter()
                .any(|b| b.name == *branch && b.worktree.is_none())
        });
        if not_checked_out && only_on_lines_of_others(p, branch, None).unwrap_or(false) {
            let _ = delete_branch(p.repo(), branch);
        }
    }
    if status(p.repo()).is_ok_and(|now| now.entries.is_empty()) {
        let _ = bring_back_load_aside(p);
    }
    untouched.sort();
    untouched
}

/// The paths of the files that differ between the commits `from` and `to`.
fn changed_between(p: &Project, from: &str, to: &str) -> Result<Vec<String>, CoreError> {
    let out = p.repo().run(
        &["diff", "--name-only", "-z", "--no-renames", from, to, "--"],
        &RunOpts::read_only(),
    )?;
    Ok(out
        .split(|&b| b == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect())
}

/// Step 7: adds the entries of `before` that the switch un-ignored to `.git/info/exclude`, as
/// anchored literal patterns, and returns those patterns.
fn keep_ignored(p: &Project, before: &[String]) -> Result<Vec<String>, CoreError> {
    if before.is_empty() {
        return Ok(Vec::new());
    }
    let untracked: Vec<String> = status(p.repo())?
        .entries
        .into_iter()
        .filter(|e| e.kind == EntryKind::Untracked)
        .map(|e| e.path)
        .collect();
    let patterns: Vec<String> = kept_ignored(before, &untracked)
        .iter()
        .filter(|entry| line_safe(entry))
        .map(|entry| exclude_pattern(entry))
        .collect();
    append_info_exclude(p.repo(), KEEP_IGNORED_COMMENT, &patterns)?;
    Ok(patterns)
}

/// Step 10: deletes the app timeline just left when it has no save of its own. A timeline just
/// started does not count as another line here: when it begins at the left one's last save, it
/// would otherwise make that timeline look empty and cost it its name.
fn tidy_left(
    p: &Project,
    origin: &LoadOrigin,
    now_on: &str,
    started: bool,
) -> Result<Option<String>, CoreError> {
    let Some(left) = origin.branch.as_deref() else {
        return Ok(None);
    };
    if !left.starts_with(APP_PREFIX) || left == now_on {
        return Ok(None);
    }
    if !only_on_lines_of_others(p, left, started.then_some(now_on))? {
        return Ok(None);
    }
    delete_branch(p.repo(), left)?;
    Ok(Some(left.to_owned()))
}

/// Whether `branch`'s last save is on the first-parent line of a local branch other than
/// `branch` and `besides`: `is_empty_app_timeline` with one branch left out.
fn only_on_lines_of_others(
    p: &Project,
    branch: &str,
    besides: Option<&str>,
) -> Result<bool, CoreError> {
    let all = list_branches(p.repo())?;
    let Some(me) = all.iter().find(|b| b.name == branch) else {
        return Ok(false);
    };
    for other in all
        .iter()
        .filter(|b| b.name != branch && Some(b.name.as_str()) != besides)
    {
        if on_first_parent_line(p.repo(), &me.tip, &other.name)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// **Tentar de novo** (spec §4.3 step 5): finishes a Load that git left part-way, from `origin`
/// (the save it came from). For each unsaved change:
/// - a tracked file that is missing, or still exactly as `origin` saved it, is written as the
///   current save has it;
/// - an untracked file exactly as `origin` saved it is deleted (its content is in that save).
///
/// Anything else (a file someone changed since) is left alone. When the folder is clean, the
/// files Load set aside come back. Returns the paths still left over.
pub fn finish_load(p: &Project, origin: &SaveId) -> Result<Vec<String>, CoreError> {
    let entries = status(p.repo())?.entries;
    if !entries.is_empty() {
        let saved = tree_oids(p, &origin.0)?;
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        let on_disk = worktree_oids(p, &paths);
        let as_saved = |path: &str| {
            saved
                .get(path)
                .is_some_and(|oid| on_disk.get(path) == Some(oid))
        };
        // Each file on its own: one still in use must not stop the others.
        for entry in &entries {
            let path = entry.path.as_str();
            match entry.kind {
                EntryKind::Deleted => {
                    let _ = restore_worktree(p.repo(), "HEAD", &literal(path));
                }
                EntryKind::Modified | EntryKind::TypeChanged if as_saved(path) => {
                    let _ = restore_worktree(p.repo(), "HEAD", &literal(path));
                }
                EntryKind::Untracked if as_saved(path) => {
                    let _ = remove_with_empty_folders(p.root(), path);
                }
                _ => {}
            }
        }
    }
    let left: Vec<String> = status(p.repo())?
        .entries
        .into_iter()
        .map(|e| e.path)
        .collect();
    if left.is_empty() {
        bring_back_load_aside(p)?;
    }
    Ok(left)
}

fn literal(path: &str) -> String {
    format!(":(top,literal){path}")
}

/// The blob id of every file in `rev`'s tree, by path.
fn tree_oids(p: &Project, rev: &str) -> Result<HashMap<String, String>, CoreError> {
    let out = p
        .repo()
        .run(&["ls-tree", "-r", "-z", rev, "--"], &RunOpts::read_only())?;
    Ok(out
        .split(|&b| b == 0)
        .filter_map(|record| {
            // `<mode> SP <type> SP <oid> TAB <path>`
            let tab = record.iter().position(|&b| b == b'\t')?;
            let meta = String::from_utf8_lossy(&record[..tab]);
            let mut fields = meta.split(' ');
            let (_, kind, oid) = (fields.next()?, fields.next()?, fields.next()?);
            (kind == "blob").then(|| {
                (
                    String::from_utf8_lossy(&record[tab + 1..]).into_owned(),
                    oid.to_owned(),
                )
            })
        })
        .collect())
}

/// How many paths go to one `hash-object`, so the command line stays short.
const HASH_CHUNK: usize = 64;

/// The blob id git would store for each of `paths` that is a plain file on disk, by path. A
/// file that can't be read is missing from the result, so it never counts as saved.
fn worktree_oids(p: &Project, paths: &[&str]) -> HashMap<String, String> {
    let files: Vec<&str> = paths
        .iter()
        .copied()
        .filter(|path| fs::symlink_metadata(p.root().join(path)).is_ok_and(|m| m.is_file()))
        .collect();
    let mut oids = HashMap::new();
    for chunk in files.chunks(HASH_CHUNK) {
        match hash_objects(p, chunk) {
            Some(found) => oids.extend(found),
            // One unreadable file fails the whole chunk: ask for each on its own.
            None => {
                for path in chunk {
                    oids.extend(hash_objects(p, std::slice::from_ref(path)).unwrap_or_default());
                }
            }
        }
    }
    oids
}

/// `git hash-object` for `files`, by path; `None` when git fails for any of them.
fn hash_objects(p: &Project, files: &[&str]) -> Option<Vec<(String, String)>> {
    let mut args = vec!["hash-object", "--"];
    args.extend(files.iter().copied());
    let out = p.repo().run(&args, &RunOpts::read_only()).ok()?;
    let printed = String::from_utf8_lossy(&out);
    let lines: Vec<&str> = printed.lines().map(str::trim).collect();
    (lines.len() == files.len()).then(|| {
        files
            .iter()
            .zip(lines)
            .map(|(path, oid)| ((*path).to_owned(), oid.to_owned()))
            .collect()
    })
}

/// Spec §4.6: for a project on a bare save (and nothing else in the way), continues the
/// timeline that ends at it, or starts an app timeline there. No file changes, so unsaved
/// changes are carried along. Returns the branch now current.
pub fn continue_from_here(p: &Project) -> Result<String, LoadError> {
    match snapshot(p).condition {
        RepoCondition::ReadOnly(blockers) if blockers == [Blocker::Detached] => {}
        _ => return Err(LoadError::NotNormal),
    }
    let Some(head) = status(p.repo())?.head_oid else {
        return Err(LoadError::NotNormal);
    };
    let timelines = list_timelines(p)?;
    let failed = |e: GitError| match e {
        GitError::Failed { stderr, .. } => LoadError::SwitchFailed {
            details: stderr.trim().to_owned(),
            untouched: Vec::new(),
        },
        other => other.into(),
    };
    match load_destination(&timelines, &SaveId(head.clone()), None) {
        Destination::Switch(branch) => {
            switch_to(p.repo(), &branch).map_err(failed)?;
            Ok(branch)
        }
        Destination::Start => {
            if prefix_taken(p, &timelines)? {
                return Err(LoadError::PrefixTaken);
            }
            let taken: Vec<String> = timelines.iter().map(|t| t.branch.clone()).collect();
            let name = new_app_branch_name(&taken, clock_seed());
            switch_new(p.repo(), &name, &head).map_err(failed)?;
            Ok(name)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        Destination, LoadError, exclude_pattern, kept_ignored, load_destination, load_with,
    };
    use crate::model::SaveId;
    use crate::project::{OpenOutcome, open_project};
    use crate::timelines::Timeline;

    /// An AI tool writes to a file the target changes, after Load checked the folder and before
    /// git switches. Git refuses ("would be overwritten") without moving, and the undo must
    /// leave that edit alone: it is the only copy of that work.
    #[test]
    fn an_edit_made_during_a_refused_switch_survives() {
        let repo = testkit::TestRepo::new();
        repo.write("a.txt", "m1");
        let m1 = repo.commit_all("m1");
        repo.write("a.txt", "m2");
        repo.write("b.txt", "b");
        let m2 = repo.commit_all("m2");
        let OpenOutcome::Opened(p) = open_project(&repo.exe(), repo.path()).unwrap() else {
            panic!("not a project");
        };

        let result = load_with(&p, &SaveId(m1), None, || {
            repo.write("a.txt", "edited by AI meanwhile");
        });
        match result {
            Err(LoadError::SwitchFailed { untouched, .. }) => assert_eq!(untouched, ["a.txt"]),
            other => panic!("expected SwitchFailed, got {other:?}"),
        }
        let read = |rel: &str| std::fs::read_to_string(repo.path().join(rel)).unwrap();
        assert_eq!(read("a.txt"), "edited by AI meanwhile");
        assert_eq!(read("b.txt"), "b");
        assert_eq!(repo.git(&["rev-parse", "HEAD"]).trim(), m2);
        assert_eq!(repo.git(&["branch", "--show-current"]).trim(), "main");
        assert_eq!(
            repo.git(&["branch", "--format=%(refname:short)"]).trim(),
            "main"
        );
    }

    fn t(branch: &str, tip: &str) -> Timeline {
        Timeline {
            branch: branch.to_owned(),
            tip: SaveId(tip.to_owned()),
            display_name: None,
            archived: false,
            main_line: false,
            current: false,
            elsewhere: None,
        }
    }

    fn main_line(mut timeline: Timeline) -> Timeline {
        timeline.main_line = true;
        timeline
    }

    fn s(oid: &str) -> SaveId {
        SaveId(oid.to_owned())
    }

    fn switch(name: &str) -> Destination {
        Destination::Switch(name.to_owned())
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn destination_continues_a_timeline_ending_at_the_save() {
        let all = [main_line(t("main", "m3")), t("exp", "e2")];
        assert_eq!(load_destination(&all, &s("e2"), None), switch("exp"));
        assert_eq!(load_destination(&all, &s("m3"), None), switch("main"));
    }

    #[test]
    fn destination_starts_mid_line() {
        let all = [main_line(t("main", "m3")), t("exp", "e2")];
        assert_eq!(load_destination(&all, &s("m2"), None), Destination::Start);
        assert_eq!(load_destination(&[], &s("m2"), None), Destination::Start);
    }

    #[test]
    fn destination_order_is_prefer_main_app_name() {
        let all = [
            t("zeta", "x"),
            t("alpha", "x"),
            t("savestates/b-b", "x"),
            t("savestates/a-a", "x"),
            main_line(t("trunk", "x")),
        ];
        assert_eq!(
            load_destination(&all, &s("x"), Some("zeta")),
            switch("zeta")
        );
        assert_eq!(
            load_destination(&all, &s("x"), Some("gone")),
            switch("trunk")
        );
        assert_eq!(load_destination(&all, &s("x"), None), switch("trunk"));
        assert_eq!(
            load_destination(&all[..4], &s("x"), None),
            switch("savestates/a-a")
        );
        assert_eq!(load_destination(&all[..2], &s("x"), None), switch("alpha"));
    }

    #[test]
    fn destination_skips_archived_current_and_elsewhere() {
        let mut archived = main_line(t("main", "x"));
        archived.archived = true;
        let mut current = t("savestates/a-a", "x");
        current.current = true;
        let mut elsewhere = t("alpha", "x");
        elsewhere.elsewhere = Some(PathBuf::from("C:/other"));
        let all = [archived, current, elsewhere];
        assert_eq!(load_destination(&all, &s("x"), None), Destination::Start);
        assert_eq!(
            load_destination(&all, &s("x"), Some("main")),
            Destination::Start
        );
        let mut all = all.to_vec();
        all.push(t("zeta", "x"));
        assert_eq!(load_destination(&all, &s("x"), None), switch("zeta"));
    }

    #[test]
    fn exclude_patterns_are_anchored_and_literal() {
        for (entry, pattern) in [
            ("node_modules/", "/node_modules/"),
            (".env", "/.env"),
            ("[id].js", r"/\[id].js"),
            ("a*b?.txt", r"/a\*b\?.txt"),
            ("#notes", r"/\#notes"),
            ("!x", r"/\!x"),
            ("end ", r"/end\ "),
            (r"back\slash", r"/back\\slash"),
            ("a b/c#!/", "/a b/c#!/"),
            ("two  ", r"/two\ \ "),
        ] {
            assert_eq!(exclude_pattern(entry), pattern, "{entry:?}");
        }
    }

    #[test]
    fn kept_ignored_maps_new_files_back_to_what_was_ignored() {
        let before = names(&["node_modules/", ".env", "logs/a.log"]);
        let after = names(&[
            "node_modules/x/y.js",
            "node_modules/z.js",
            ".env",
            "src/new.txt",
        ]);
        assert_eq!(
            kept_ignored(&before, &after),
            names(&[".env", "node_modules/"])
        );
        assert!(kept_ignored(&names(&["node_modules/"]), &names(&["node_modules_x"])).is_empty());
        assert!(kept_ignored(&names(&["dist"]), &names(&["dist/a.js"])).is_empty());
    }
}
