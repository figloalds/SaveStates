//! SaveStates domain model and the operations built on top of `savestates-git`.
#![forbid(unsafe_code)]

mod diff;
mod error;
pub mod gitignore;
mod model;
mod project;
mod safety;
mod timeline;

pub use diff::{
    CONTEXT_LINES, DiffBlock, DiffLine, DiffResult, DiffRow, DiffView, MAX_DIFF_LINES, RowKind,
    Seg, side_by_side,
};
pub use error::CoreError;
pub use model::{
    Change, ChangeKind, FileContent, MAX_DIFF_BYTES, ProjectSnapshot, RepoCondition, SaveId,
    classify,
};
pub use project::{
    GITIGNORE_TEMPLATE, OpenOutcome, Project, open_project, snapshot, start_saving_folder,
};
pub use safety::{
    Decision, Flag, FlagAction, FlagReason, JUNK_DIRS, allowed_actions, apply_decisions,
    find_secret_line, is_secret_name, scan,
};
pub use savestates_git::Blocker;
pub use timeline::{
    HISTORY_PAGE, SaveState, Side, changes_in_save, files_at_save, read_file, timeline_page,
};
