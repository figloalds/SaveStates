//! What state a repository is in: unfinished operations that make saving unsafe, and its timelines.

use std::path::PathBuf;

use crate::runner::{GitError, Repo, RunOpts};
use crate::status::{EntryKind, Status};

/// Why a repository can only be looked at, not saved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blocker {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Bisect,
    Detached,
    Conflicts,
    Bare,
}

/// Lists every reason `status` describes a repository that is mid-operation.
///
/// [`Blocker::Bare`] is never returned here: a bare repository has no work tree to report on,
/// so callers detect it separately (see `is_bare`) and skip `status` entirely.
pub fn blockers(repo: &Repo, status: &Status) -> Result<Vec<Blocker>, GitError> {
    let out = repo.run(&["rev-parse", "--absolute-git-dir"], &RunOpts::read_only())?;
    let git_dir = PathBuf::from(String::from_utf8_lossy(&out).trim_end_matches(['\r', '\n']));

    let in_progress = [
        ("MERGE_HEAD", Blocker::Merge),
        ("rebase-merge", Blocker::Rebase),
        ("rebase-apply", Blocker::Rebase),
        ("CHERRY_PICK_HEAD", Blocker::CherryPick),
        ("REVERT_HEAD", Blocker::Revert),
        ("BISECT_LOG", Blocker::Bisect),
    ];
    let mut found = Vec::new();
    for (marker, blocker) in in_progress {
        if git_dir.join(marker).try_exists()? && !found.contains(&blocker) {
            found.push(blocker);
        }
    }
    if status.branch.is_none() && status.head_oid.is_some() {
        found.push(Blocker::Detached);
    }
    if status
        .entries
        .iter()
        .any(|entry| entry.kind == EntryKind::Unmerged)
    {
        found.push(Blocker::Conflicts);
    }
    Ok(found)
}

/// True when the repository has more than one local branch (more than one "timeline").
pub fn other_branches_exist(repo: &Repo) -> Result<bool, GitError> {
    let out = repo.run(
        &["for-each-ref", "--format=%(refname)", "refs/heads"],
        &RunOpts::read_only(),
    )?;
    Ok(out.split(|&b| b == b'\n').filter(|r| !r.is_empty()).count() > 1)
}
