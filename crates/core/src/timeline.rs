//! Reading a project's history, and the files as they were in a past save ("peeking").

use std::fs;
use std::io;
use std::path::{Component, Path};

use chrono::{DateTime, FixedOffset};
use savestates_git::{
    LogEntry, blob, blob_size, first_parent, log_first_parent, ls_tree_names, name_status,
};

use crate::error::CoreError;
use crate::model::{Change, FileContent, MAX_DIFF_BYTES, SaveId, classify};
use crate::project::{Project, into_change};

/// How many saves the timeline loads at a time.
pub const HISTORY_PAGE: usize = 500;

/// One point on the timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveState {
    pub id: SaveId,
    pub name: String,
    /// When the save was first written (git's author time), in the time zone it was written in.
    pub time: DateTime<FixedOffset>,
    /// When the save was committed (git's committer time), in the committer's time zone. Differs
    /// from `time` when the save was amended, rebased or cherry-picked.
    pub committed: DateTime<FixedOffset>,
    /// How many files the save changed compared to the one before it.
    pub files_changed: u32,
}

/// Which version of a file to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Side {
    /// The file as it was in this save.
    Save(SaveId),
    /// The file as it was just before this save (the save's first parent).
    Before(SaveId),
    /// The file in the project folder right now, saved or not.
    Now,
}

/// A page of the current timeline, newest save first. Merged-in work shows as the one save
/// that merged it. Empty before the first save.
pub fn timeline_page(p: &Project, skip: usize, limit: usize) -> Result<Vec<SaveState>, CoreError> {
    Ok(log_first_parent(p.repo(), "HEAD", skip, limit)?
        .into_iter()
        .map(into_save_state)
        .collect())
}

pub(crate) fn into_save_state(entry: LogEntry) -> SaveState {
    SaveState {
        time: parse_time(&entry.author_time, entry.author_unix),
        committed: parse_time(&entry.committer_time, entry.committer_unix),
        id: SaveId(entry.oid),
        name: entry.subject,
        files_changed: entry.files,
    }
}

/// The date git printed, in the time zone it was made in. A commit with a corrupt time zone has
/// no such date, so it falls back to its Unix timestamp at UTC, and to the Unix epoch if that is
/// unusable too: one odd commit must not hide the rest of the history.
fn parse_time(iso: &str, unix: Option<i64>) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(iso)
        .ok()
        .or_else(|| {
            let utc = DateTime::from_timestamp(unix?, 0)?;
            Some(utc.fixed_offset())
        })
        .unwrap_or_else(|| DateTime::UNIX_EPOCH.fixed_offset())
}

/// What a save changed compared to the save before it (every file is new in the first save).
pub fn changes_in_save(p: &Project, id: &SaveId) -> Result<Vec<Change>, CoreError> {
    Ok(name_status(p.repo(), &id.0)?
        .into_iter()
        .map(into_change)
        .collect())
}

/// Every file that existed in a save, as paths relative to the project folder.
pub fn files_at_save(p: &Project, id: &SaveId) -> Result<Vec<String>, CoreError> {
    Ok(ls_tree_names(p.repo(), &id.0)?)
}

/// Reads `path` (relative to the project folder, as git reports it) on one `side`.
///
/// A file bigger than [`MAX_DIFF_BYTES`] is reported as [`FileContent::TooLarge`] without
/// being read, unless `force` is set.
pub fn read_file(
    p: &Project,
    side: &Side,
    path: &str,
    force: bool,
) -> Result<FileContent, CoreError> {
    if !stays_inside_project(path) {
        return Err(CoreError::NotFound(path.to_owned()));
    }
    match side {
        Side::Now => read_working_file(p, path, force),
        Side::Save(id) => read_saved_file(p, &id.0, path, force),
        Side::Before(id) => match first_parent(p.repo(), &id.0)? {
            Some(parent) => read_saved_file(p, &parent, path, force),
            None => Ok(FileContent::Missing),
        },
    }
}

/// True for a non-empty path made only of plain names, so joining it to the project folder
/// cannot leave that folder.
pub(crate) fn stays_inside_project(path: &str) -> bool {
    let mut components = Path::new(path).components().peekable();
    components.peek().is_some() && components.all(|c| matches!(c, Component::Normal(_)))
}

fn read_saved_file(
    p: &Project,
    rev: &str,
    path: &str,
    force: bool,
) -> Result<FileContent, CoreError> {
    let Some(bytes) = blob_size(p.repo(), rev, path)? else {
        return Ok(FileContent::Missing);
    };
    if bytes > MAX_DIFF_BYTES && !force {
        return Ok(FileContent::TooLarge { bytes });
    }
    Ok(blob(p.repo(), rev, path)?.map_or(FileContent::Missing, classify))
}

fn read_working_file(p: &Project, path: &str, force: bool) -> Result<FileContent, CoreError> {
    let full = p.root().join(path);
    let meta = match fs::metadata(&full) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(FileContent::Missing),
        Err(e) => return Err(e.into()),
    };
    if !meta.is_file() {
        return Ok(FileContent::Missing);
    }
    if meta.len() > MAX_DIFF_BYTES && !force {
        return Ok(FileContent::TooLarge { bytes: meta.len() });
    }
    match fs::read(&full) {
        Ok(bytes) => Ok(classify(bytes)),
        // Deleted between the size check and the read.
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(FileContent::Missing),
        Err(e) => Err(e.into()),
    }
}
