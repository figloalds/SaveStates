//! History, read from `git log`: the first-parent line of saves and what each one changed.

use crate::runner::{GitError, Repo, RunOpts};
use crate::status::{EntryKind, StatusEntry};

/// Starts every record in `log_first_parent`'s output (ASCII "record separator").
const RECORD_START: u8 = 0x1e;
/// Separates the fields inside a record's header (ASCII "unit separator").
const FIELD_SEP: u8 = 0x1f;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub oid: String,
    /// Author date in strict ISO 8601 (`%aI`), e.g. `2026-11-01T17:28:18-03:00`. Exactly what
    /// git printed, so for a commit with a corrupt time zone it is not valid ISO 8601 (or not
    /// a date at all); [`LogEntry::author_unix`] is the dependable alternative.
    pub author_time: String,
    /// Author time in seconds since the Unix epoch (`%at`); `None` when git printed no
    /// usable number.
    pub author_unix: Option<i64>,
    pub subject: String,
    /// How many files the commit changed compared to its first parent.
    pub files: u32,
}

/// The commits reachable from `rev` along first parents only, newest first.
///
/// `skip` and `limit` page through them. When `rev` is `HEAD` and the branch is unborn there is
/// nothing to list, so the result is empty rather than an error.
pub fn log_first_parent(
    repo: &Repo,
    rev: &str,
    skip: usize,
    limit: usize,
) -> Result<Vec<LogEntry>, GitError> {
    let skip = format!("--skip={skip}");
    let limit = limit.to_string();
    let result = repo.run(
        &[
            "log",
            "--first-parent",
            "-z",
            "--name-only",
            "--format=%x1e%H%x1f%aI%x1f%at%x1f%s",
            &skip,
            "-n",
            &limit,
            rev,
            // A file named like `rev` must not make the argument ambiguous.
            "--",
        ],
        &RunOpts::read_only(),
    );
    match result {
        Ok(out) => Ok(parse_log(&out)),
        // `log` words an unborn branch like any other bad revision, so ask separately.
        Err(e) => {
            let unborn = rev == "HEAD"
                && matches!(e, GitError::Failed { .. })
                && !resolves_to_commit(repo, rev)?;
            if unborn { Ok(Vec::new()) } else { Err(e) }
        }
    }
}

/// True when `rev` names an existing commit.
fn resolves_to_commit(repo: &Repo, rev: &str) -> Result<bool, GitError> {
    let object = format!("{rev}^{{commit}}");
    match repo.run(
        &["rev-parse", "--verify", "--quiet", &object],
        &RunOpts::read_only(),
    ) {
        Ok(_) => Ok(true),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Parses `%x1e<oid>%x1f<%aI>%x1f<%at>%x1f<subject>` records, each followed by `NUL \n` and the
/// NUL-terminated names of the files it changed. Malformed records are skipped.
fn parse_log(bytes: &[u8]) -> Vec<LogEntry> {
    bytes
        .split(|&b| b == RECORD_START)
        .filter_map(parse_record)
        .collect()
}

fn parse_record(record: &[u8]) -> Option<LogEntry> {
    // In `-z` mode the format ends at a NUL; the changed files follow, after one newline.
    let (header, files) = match record.iter().position(|&b| b == 0) {
        Some(end) => (&record[..end], &record[end + 1..]),
        None => (record, &[][..]),
    };
    let files = files.strip_prefix(b"\n").unwrap_or(files);

    let mut fields = header
        .splitn(4, |&b| b == FIELD_SEP)
        .map(String::from_utf8_lossy);
    let oid = fields.next().filter(|oid| !oid.is_empty())?;
    let author_time = fields.next()?;
    let author_unix = fields.next()?.parse().ok();
    let subject = fields.next()?;

    let files = files
        .split(|&b| b == 0)
        .filter(|name| !name.is_empty())
        .count();
    Some(LogEntry {
        oid: oid.into_owned(),
        author_time: author_time.into_owned(),
        author_unix,
        subject: subject.into_owned(),
        files: u32::try_from(files).unwrap_or(u32::MAX),
    })
}

/// What the commit `oid` changed compared to its first parent (everything, for a root commit).
pub fn name_status(repo: &Repo, oid: &str) -> Result<Vec<StatusEntry>, GitError> {
    let out = repo.run(
        &[
            "log",
            "-1",
            "-z",
            "--name-status",
            "-M",
            "--diff-merges=first-parent",
            "--format=",
            oid,
            "--",
        ],
        &RunOpts::read_only(),
    )?;
    Ok(parse_name_status(&out))
}

/// Parses `<status> NUL <path> NUL` records; renames and copies carry two paths, old then new.
fn parse_name_status(bytes: &[u8]) -> Vec<StatusEntry> {
    let mut tokens = bytes
        .split(|&b| b == 0)
        .filter(|token| !token.is_empty())
        .map(String::from_utf8_lossy);
    let mut entries = Vec::new();

    while let Some(status) = tokens.next() {
        let Some(first) = tokens.next() else { break };
        let letter = status.chars().next();
        let entry = if matches!(letter, Some('R' | 'C')) {
            let Some(second) = tokens.next() else { break };
            let renamed = letter == Some('R');
            StatusEntry {
                path: second.into_owned(),
                kind: if renamed {
                    EntryKind::Renamed
                } else {
                    EntryKind::Added
                },
                orig_path: renamed.then(|| first.into_owned()),
            }
        } else {
            StatusEntry {
                path: first.into_owned(),
                kind: match letter {
                    Some('A') => EntryKind::Added,
                    Some('D') => EntryKind::Deleted,
                    Some('T') => EntryKind::TypeChanged,
                    _ => EntryKind::Modified,
                },
                orig_path: None,
            }
        };
        entries.push(entry);
    }
    entries
}

/// The first parent of commit `oid`, or `None` for a root commit.
pub fn first_parent(repo: &Repo, oid: &str) -> Result<Option<String>, GitError> {
    let out = repo.run(
        &["rev-list", "--parents", "-n", "1", oid, "--"],
        &RunOpts::read_only(),
    )?;
    // The line is `<oid> <parent> <parent>...`.
    Ok(String::from_utf8_lossy(&out)
        .split_whitespace()
        .nth(1)
        .map(str::to_owned))
}
