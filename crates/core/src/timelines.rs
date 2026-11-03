//! Timelines (git branches) and the saves drawn on them (spec §3, §6).

use std::collections::HashSet;
use std::path::PathBuf;

use savestates_git::{
    BranchInfo, branches_containing, config_get, config_set, delete_branch, first_parent_count,
    head_branch, head_commit, list_branches, log_first_parent, log_graph, on_first_parent_line,
    own_first_parent_saves, status,
};

use crate::error::CoreError;
use crate::model::SaveId;
use crate::project::Project;
use crate::timeline::{SaveState, into_save_state};

/// The git setting that remembers which branch is the project's main line (spec §3.2).
pub const MAIN_LINE_KEY: &str = "savestates.main-line";

/// The prefix of every branch the app makes for itself (`savestates/<words>`, spec §5.1).
pub const APP_PREFIX: &str = "savestates/";

/// One local branch, seen as a timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timeline {
    /// The git branch name, e.g. `main` or `savestates/tarde-de-domingo`.
    pub branch: String,
    /// The branch's last save.
    pub tip: SaveId,
    /// The name the user gave it (`branch.<name>.description`), if any.
    pub display_name: Option<String>,
    pub archived: bool,
    /// Whether this is the project's main line (spec §3.2).
    pub main_line: bool,
    /// Whether the project folder is on this timeline.
    pub current: bool,
    /// The folder of another git worktree this branch is checked out in.
    pub elsewhere: Option<PathBuf>,
}

impl Timeline {
    /// Whether the app made this timeline (its git name starts with [`APP_PREFIX`]).
    pub fn app_made(&self) -> bool {
        self.branch.starts_with(APP_PREFIX)
    }
}

/// A save together with the saves it was made on top of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphSave {
    pub save: SaveState,
    /// The first parent is the save before it on its own line; more than one means a merge.
    pub parents: Vec<SaveId>,
}

/// Everything the timeline screen needs to draw every timeline at once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimelinesView {
    /// Every local branch, by name.
    pub timelines: Vec<Timeline>,
    /// The save the project folder is on; `None` before the first save.
    pub head: Option<SaveId>,
    /// Whether the project folder is on a save rather than on a timeline.
    pub detached: bool,
    /// The loaded saves of every branch, newest first, as `log_graph` lists them.
    pub saves: Vec<GraphSave>,
    /// Whether older saves exist beyond the loaded ones.
    pub more_available: bool,
}

/// Every local branch as a timeline, without history: the main line is resolved (and
/// remembered at first sight) and app-made timelines are named after their first own save.
/// A bare repository is only looked at: there, nothing is remembered or written.
pub fn list_timelines(p: &Project) -> Result<Vec<Timeline>, CoreError> {
    let (current, _) = where_head_is(p)?;
    timelines_of(p, current.as_deref())
}

/// The branch the project is on (`None` when on a bare save) and the save HEAD is on (`None`
/// before the first save). A bare repository has no work tree, so it is read without `status`.
fn where_head_is(p: &Project) -> Result<(Option<String>, Option<String>), CoreError> {
    if p.is_bare() {
        return Ok((head_branch(p.repo())?, head_commit(p.repo())?));
    }
    let st = status(p.repo())?;
    Ok((st.branch, st.head_oid))
}

fn timelines_of(p: &Project, current: Option<&str>) -> Result<Vec<Timeline>, CoreError> {
    let branches = list_branches(p.repo())?;
    let main = main_line(p, &branches, current)?;
    let mut timelines = Vec::with_capacity(branches.len());
    for b in &branches {
        let mut display_name = b.description.clone();
        if display_name.is_none() && b.name.starts_with(APP_PREFIX) {
            let others: Vec<String> = branches
                .iter()
                .filter(|o| o.name != b.name)
                .map(|o| o.tip.clone())
                .collect();
            let own = own_first_parent_saves(p.repo(), &b.tip, &others)?;
            if let Some(first) = own.last()
                && let Some(entry) = log_first_parent(p.repo(), first, 0, 1)?.into_iter().next()
            {
                // Only shown, not written, in a bare repository (which is only looked at).
                if !p.is_bare() {
                    let key = format!("branch.{}.description", b.name);
                    config_set(p.repo(), &key, &entry.subject)?;
                }
                display_name = Some(entry.subject);
            }
        }
        let is_current = current == Some(b.name.as_str());
        timelines.push(Timeline {
            branch: b.name.clone(),
            tip: SaveId(b.tip.clone()),
            display_name,
            archived: b.archived,
            main_line: main.as_deref() == Some(b.name.as_str()),
            current: is_current,
            elsewhere: if is_current { None } else { b.worktree.clone() },
        });
    }
    Ok(timelines)
}

/// Spec §3.2: the remembered main line, else the current non-app branch, `main`, `master`, or
/// the longest line. A pick made without a stored value is stored, except in a bare repository
/// (which is only looked at).
fn main_line(
    p: &Project,
    branches: &[BranchInfo],
    current: Option<&str>,
) -> Result<Option<String>, CoreError> {
    let exists = |name: &str| branches.iter().any(|b| b.name == name);
    let stored = config_get(p.repo(), MAIN_LINE_KEY)?;
    if let Some(name) = stored.as_deref().filter(|n| exists(n)) {
        return Ok(Some(name.to_owned()));
    }
    let mut pick = None;
    if stored.is_none() {
        pick = current
            .filter(|c| !c.starts_with(APP_PREFIX) && exists(c))
            .map(str::to_owned);
    }
    if pick.is_none() {
        pick = ["main", "master"]
            .into_iter()
            .find(|n| exists(n))
            .map(str::to_owned);
    }
    if pick.is_none() {
        let mut best: Option<(u64, &str)> = None;
        for b in branches {
            let n = first_parent_count(p.repo(), &b.name)?;
            // Branches are sorted by name, so a strict comparison keeps the first of a tie.
            if best.is_none_or(|(m, _)| n > m) {
                best = Some((n, &b.name));
            }
        }
        pick = best.map(|(_, n)| n.to_owned());
    }
    if stored.is_none()
        && !p.is_bare()
        && let Some(name) = &pick
    {
        config_set(p.repo(), MAIN_LINE_KEY, name)?;
    }
    Ok(pick)
}

/// The timelines plus the newest `limit` saves of all of them.
pub fn read_timelines(p: &Project, limit: usize) -> Result<TimelinesView, CoreError> {
    let (branch, head) = where_head_is(p)?;
    let timelines = timelines_of(p, branch.as_deref())?;
    let saves = read_older_saves(p, 0, limit)?;
    Ok(TimelinesView {
        timelines,
        detached: branch.is_none() && head.is_some(),
        head: head.map(SaveId),
        more_available: saves.len() == limit,
        saves,
    })
}

/// A page of saves further back than [`read_timelines`] loaded.
pub fn read_older_saves(
    p: &Project,
    skip: usize,
    limit: usize,
) -> Result<Vec<GraphSave>, CoreError> {
    Ok(log_graph(p.repo(), skip, limit)?
        .into_iter()
        .map(|entry| {
            let parents = entry.parents.iter().cloned().map(SaveId).collect();
            GraphSave {
                save: into_save_state(entry),
                parents,
            }
        })
        .collect())
}

/// An app-made timeline with no saves of its own: its last save is already on another
/// timeline's line. Archived timelines do not count as other timelines: they are not drawn, so
/// they make no lane of their own.
pub fn is_empty_app_timeline(p: &Project, branch: &str) -> Result<bool, CoreError> {
    if !branch.starts_with(APP_PREFIX) {
        return Ok(false);
    }
    only_on_lines_of_others(p, branch, None)
}

/// Whether `branch`'s last save is on the first-parent line of a local branch other than
/// `branch`, `besides` and the archived ones: [`is_empty_app_timeline`] with one more branch
/// left out, and without the check of the name.
pub(crate) fn only_on_lines_of_others(
    p: &Project,
    branch: &str,
    besides: Option<&str>,
) -> Result<bool, CoreError> {
    let branches = list_branches(p.repo())?;
    let Some(me) = branches.iter().find(|b| b.name == branch) else {
        return Ok(false);
    };
    on_lines_of_others(p, me, &archived_names(&branches), besides)
}

/// The names of the archived branches among `branches`.
fn archived_names(branches: &[BranchInfo]) -> HashSet<String> {
    branches
        .iter()
        .filter(|b| b.archived)
        .map(|b| b.name.clone())
        .collect()
}

/// Whether `me`'s tip is on the first-parent line of a branch that exists now, other than `me`,
/// `besides` and those in `archived`. Only the branches whose history holds the tip are walked,
/// which usually is none at all.
fn on_lines_of_others(
    p: &Project,
    me: &BranchInfo,
    archived: &HashSet<String>,
    besides: Option<&str>,
) -> Result<bool, CoreError> {
    for other in branches_containing(p.repo(), &me.tip)? {
        if other == me.name || Some(other.as_str()) == besides || archived.contains(&other) {
            continue;
        }
        if on_first_parent_line(p.repo(), &me.tip, &other)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Deletes every empty app timeline that is not current or checked out elsewhere (for example
/// one left behind by a crash after Load). Returns their names.
pub fn sweep_empty_timelines(p: &Project) -> Result<Vec<String>, CoreError> {
    let current = status(p.repo())?.branch;
    let branches = list_branches(p.repo())?;
    let archived = archived_names(&branches);
    let mut removed = Vec::new();
    for b in branches {
        if !b.name.starts_with(APP_PREFIX)
            || b.worktree.is_some()
            || current.as_deref() == Some(b.name.as_str())
        {
            continue;
        }
        // Checked one by one against the branches left (the ones holding its tip are listed
        // afresh each time), so twins never delete each other's only reference.
        if on_lines_of_others(p, &b, &archived, None)? {
            delete_branch(p.repo(), &b.name)?;
            removed.push(b.name);
        }
    }
    Ok(removed)
}
