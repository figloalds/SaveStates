//! Branches ("timelines"): listing them with their settings, moving between them, and the
//! first-parent questions the timeline view asks.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::runner::{GitError, Repo, RunOpts};
use crate::write::config_get_regexp;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchInfo {
    /// The short name, e.g. `main` or `savestates/graceful-elephant`.
    pub name: String,
    pub tip: String,
    /// Where the branch is checked out (this folder or another worktree), if anywhere.
    pub worktree: Option<PathBuf>,
    /// `branch.<name>.description`, without its trailing newline.
    pub description: Option<String>,
    /// `branch.<name>.savestates-archived` is `true`.
    pub archived: bool,
}

/// The commit HEAD is on; `None` before the first commit. Works without a work tree (a bare
/// repository), unlike `status`.
pub fn head_commit(repo: &Repo) -> Result<Option<String>, GitError> {
    match repo.run(
        &["rev-parse", "--verify", "-q", "HEAD^{commit}"],
        &RunOpts::read_only(),
    ) {
        Ok(out) => Ok(Some(String::from_utf8_lossy(&out).trim().to_owned())),
        // `-q` makes an unresolvable HEAD a silent exit code 1.
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The local branch HEAD is on (it may have no commit yet); `None` when HEAD is detached. Works
/// without a work tree (a bare repository), unlike `status`.
pub fn head_branch(repo: &Repo) -> Result<Option<String>, GitError> {
    match repo.run(&["symbolic-ref", "-q", "HEAD"], &RunOpts::read_only()) {
        Ok(out) => {
            let full = String::from_utf8_lossy(&out).trim().to_owned();
            Ok(full.strip_prefix("refs/heads/").map(str::to_owned))
        }
        // `-q` makes a detached HEAD a silent exit code 1.
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Every local branch, sorted by name.
pub fn list_branches(repo: &Repo) -> Result<Vec<BranchInfo>, GitError> {
    let out = repo.run(
        &[
            "for-each-ref",
            "refs/heads",
            "--format=%(refname:strip=2)%00%(objectname)%00%(worktreepath)",
        ],
        &RunOpts::read_only(),
    )?;

    // Keys are `branch.<name>.<variable>`; names hold dots and slashes, variables hold neither.
    let mut descriptions = HashMap::new();
    let mut archived = HashMap::new();
    for (key, value) in config_get_regexp(repo, r"^branch\.")? {
        let Some((name, variable)) = key
            .strip_prefix("branch.")
            .and_then(|rest| rest.rsplit_once('.'))
        else {
            continue;
        };
        match variable {
            "description" => {
                descriptions.insert(
                    name.to_owned(),
                    value.trim_end_matches(['\r', '\n']).to_owned(),
                );
            }
            "savestates-archived" => {
                archived.insert(name.to_owned(), value == "true");
            }
            _ => {}
        }
    }

    let mut branches: Vec<BranchInfo> = String::from_utf8_lossy(&out)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\0');
            let name = fields.next().filter(|n| !n.is_empty())?;
            let tip = fields.next()?;
            let worktree = fields.next().filter(|w| !w.is_empty()).map(PathBuf::from);
            Some(BranchInfo {
                name: name.to_owned(),
                tip: tip.to_owned(),
                worktree,
                description: descriptions.remove(name),
                archived: archived.get(name).copied().unwrap_or(false),
            })
        })
        .collect();
    branches.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(branches)
}

/// Moves the project onto the existing branch.
pub fn switch_to(repo: &Repo, branch: &str) -> Result<(), GitError> {
    repo.run(&["switch", "-q", "--no-guess", branch], &RunOpts::default())?;
    Ok(())
}

/// Creates `branch` at `at` and moves the project onto it.
pub fn switch_new(repo: &Repo, branch: &str, at: &str) -> Result<(), GitError> {
    repo.run(
        &["switch", "-q", "--no-track", "-c", branch, at],
        &RunOpts::default(),
    )?;
    Ok(())
}

/// Creates `branch` at `at` without moving.
pub fn create_branch(repo: &Repo, branch: &str, at: &str) -> Result<(), GitError> {
    repo.run(&["branch", "--no-track", branch, at], &RunOpts::default())?;
    Ok(())
}

/// Deletes the branch whether or not it is merged; git also drops its settings.
pub fn delete_branch(repo: &Repo, branch: &str) -> Result<(), GitError> {
    repo.run(&["branch", "-D", branch], &RunOpts::default())?;
    Ok(())
}

/// True when `oid` is on the first-parent line of `of` (a branch or an oid).
pub fn on_first_parent_line(repo: &Repo, oid: &str, of: &str) -> Result<bool, GitError> {
    // Everything `oid`'s parents reach is left out, so the walk stops right below `oid`. A
    // root commit has no parents: `<root>^@` names nothing, and should a git refuse it, the
    // walk is made without the exclusion.
    let parents = format!("{oid}^@");
    let out = match repo.run(
        &["rev-list", "--first-parent", of, "--not", &parents, "--"],
        &RunOpts::read_only(),
    ) {
        Ok(out) => out,
        Err(GitError::Failed { .. }) if crate::log::first_parent(repo, oid)?.is_none() => repo
            .run(
                &["rev-list", "--first-parent", of, "--"],
                &RunOpts::read_only(),
            )?,
        Err(e) => return Err(e),
    };
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .any(|line| line.trim() == oid))
}

/// The local branches whose history holds `oid` (its own branch included), by short name. Only
/// these can have `oid` on their first-parent line.
pub fn branches_containing(repo: &Repo, oid: &str) -> Result<Vec<String>, GitError> {
    let out = repo.run(
        &[
            "for-each-ref",
            "--contains",
            oid,
            "--format=%(refname:strip=2)",
            "refs/heads",
        ],
        &RunOpts::read_only(),
    )?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect())
}

/// The saves on `tip`'s first-parent line that none of `others` (tip oids) can reach, newest
/// first.
pub fn own_first_parent_saves(
    repo: &Repo,
    tip: &str,
    others: &[String],
) -> Result<Vec<String>, GitError> {
    let mut args = vec!["rev-list", "--first-parent", tip];
    if !others.is_empty() {
        args.push("--not");
        args.extend(others.iter().map(String::as_str));
    }
    args.push("--");
    let out = repo.run(&args, &RunOpts::read_only())?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect())
}

/// How many commits are on `rev`'s first-parent line.
pub fn first_parent_count(repo: &Repo, rev: &str) -> Result<u64, GitError> {
    let out = repo.run(
        &["rev-list", "--count", "--first-parent", rev, "--"],
        &RunOpts::read_only(),
    )?;
    Ok(String::from_utf8_lossy(&out).trim().parse().unwrap_or(0))
}
