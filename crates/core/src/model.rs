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

/// Files bigger than this are not loaded for a diff unless the user asks for it (1 MB).
pub const MAX_DIFF_BYTES: u64 = 1_048_576;

/// How many leading bytes are searched for a NUL byte when telling text from binary.
const BINARY_SNIFF_BYTES: usize = 8_000;

/// A file as it was at one point in time, ready for a diff or a preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileContent {
    /// The file does not exist at that point.
    Missing,
    Binary,
    Text(String),
    /// Not read because it is bigger than [`MAX_DIFF_BYTES`].
    TooLarge {
        bytes: u64,
    },
}

/// Sorts `bytes` into [`FileContent::Binary`] (a NUL byte in the first 8,000 bytes) or
/// [`FileContent::Text`], replacing any invalid UTF-8 with U+FFFD.
pub fn classify(bytes: Vec<u8>) -> FileContent {
    if bytes.iter().take(BINARY_SNIFF_BYTES).any(|&b| b == 0) {
        return FileContent::Binary;
    }
    FileContent::Text(
        String::from_utf8(bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
    )
}
