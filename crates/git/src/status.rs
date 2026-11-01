//! `git status --porcelain=v2 -z --branch --untracked-files=all`, parsed.

use crate::runner::{GitError, Repo, RunOpts};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntryKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Unmerged,
    TypeChanged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    pub kind: EntryKind,
    /// The previous path, for [`EntryKind::Renamed`].
    pub orig_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// `None` for an unborn branch (`(initial)`).
    pub head_oid: Option<String>,
    /// `None` for a detached HEAD (`(detached)`).
    pub branch: Option<String>,
    pub entries: Vec<StatusEntry>,
}

/// Runs `git status` and parses it. Read-only: takes no optional locks.
pub fn status(repo: &Repo) -> Result<Status, GitError> {
    let out = repo.run(
        &[
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
        ],
        &RunOpts::read_only(),
    )?;
    Ok(parse_status_v2(&out))
}

/// Parses NUL-separated `--porcelain=v2 -z --branch` output. Unknown or malformed records are skipped.
pub fn parse_status_v2(bytes: &[u8]) -> Status {
    let mut status = Status {
        head_oid: None,
        branch: None,
        entries: Vec::new(),
    };
    let mut records = bytes
        .split(|&b| b == 0)
        .filter(|record| !record.is_empty())
        .map(String::from_utf8_lossy);

    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("# ") {
            parse_header(header, &mut status);
            continue;
        }
        // Ordinary, rename/copy and unmerged records have a fixed number of space-separated
        // fields before the path; the path itself may contain spaces, so it is the unsplit rest.
        let entry = match record.split_once(' ') {
            Some(("1", rest)) => parse_ordinary(rest),
            Some(("2", rest)) => {
                // The original path follows as its own NUL-terminated record.
                let orig_path = records.next().map(|orig| orig.into_owned());
                parse_renamed(rest, orig_path)
            }
            Some(("u", rest)) => parse_unmerged(rest),
            Some(("?", path)) => Some(StatusEntry {
                path: path.to_owned(),
                kind: EntryKind::Untracked,
                orig_path: None,
            }),
            _ => None,
        };
        status.entries.extend(entry);
    }
    status
}

fn parse_header(header: &str, status: &mut Status) {
    match header.split_once(' ') {
        Some(("branch.oid", "(initial)")) => status.head_oid = None,
        Some(("branch.oid", oid)) => status.head_oid = Some(oid.to_owned()),
        Some(("branch.head", "(detached)")) => status.branch = None,
        Some(("branch.head", name)) => status.branch = Some(name.to_owned()),
        _ => {}
    }
}

/// `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>`; `rest` starts at `<XY>`.
fn parse_ordinary(rest: &str) -> Option<StatusEntry> {
    let mut fields = rest.splitn(8, ' ');
    let xy = fields.next()?;
    let path = fields.nth(6)?;
    Some(StatusEntry {
        path: path.to_owned(),
        kind: ordinary_kind(xy),
        orig_path: None,
    })
}

/// `2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>`; `rest` starts at `<XY>`.
fn parse_renamed(rest: &str, orig_path: Option<String>) -> Option<StatusEntry> {
    let path = rest.splitn(9, ' ').nth(8)?;
    Some(StatusEntry {
        path: path.to_owned(),
        kind: EntryKind::Renamed,
        orig_path,
    })
}

/// `u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>`; `rest` starts at `<XY>`.
fn parse_unmerged(rest: &str) -> Option<StatusEntry> {
    let path = rest.splitn(10, ' ').nth(9)?;
    Some(StatusEntry {
        path: path.to_owned(),
        kind: EntryKind::Unmerged,
        orig_path: None,
    })
}

/// Collapses the staged (X) and unstaged (Y) letters into one kind: D > A > M > T.
fn ordinary_kind(xy: &str) -> EntryKind {
    let has = |letter: char| xy.contains(letter);
    if has('D') {
        EntryKind::Deleted
    } else if has('A') {
        EntryKind::Added
    } else if has('M') {
        EntryKind::Modified
    } else if has('T') {
        EntryKind::TypeChanged
    } else {
        EntryKind::Modified
    }
}
