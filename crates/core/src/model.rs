//! The domain model the UI works with, free of git vocabulary.

use savestates_git::Blocker;

/// A save, identified by its commit id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SaveId(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Changed,
    New,
    Deleted,
    Renamed { from: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
}

/// Whether a project can be saved to, only looked at, or cannot be read at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoCondition {
    Normal,
    ReadOnly(Vec<Blocker>),
    Broken { details: String },
}

/// Everything the main screen shows about a project at one moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectSnapshot {
    pub condition: RepoCondition,
    /// The current timeline (branch); `None` when detached, bare or broken.
    pub timeline_name: Option<String>,
    /// The latest save; `None` before the first save, or when bare or broken.
    pub head: Option<SaveId>,
    pub changes: Vec<Change>,
    pub other_timelines: bool,
}
