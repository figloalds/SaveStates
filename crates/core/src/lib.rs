//! SaveStates domain model and the operations built on top of `savestates-git`.
#![forbid(unsafe_code)]

mod diff;
mod error;
pub mod gitignore;
mod lanes;
mod load;
mod model;
mod names;
mod project;
mod recovery;
mod restore;
mod safety;
mod save;
pub mod settings;
mod timeline;
mod timelines;

pub use diff::{
    CONTEXT_LINES, DiffBlock, DiffLine, DiffResult, DiffRow, DiffView, MAX_DIFF_LINES, RowKind,
    Seg, side_by_side,
};
pub use error::CoreError;
pub use lanes::{Lane, LaneLayout, LaneOwner, Tag, assign_lanes};
pub use load::{
    Destination, KEEP_IGNORED_COMMENT, LoadError, LoadOrigin, LoadOutcome, continue_from_here,
    exclude_pattern, finish_load, kept_ignored, load, load_destination,
};
pub use model::{
    Change, ChangeKind, FileContent, MAX_DIFF_BYTES, ProjectSnapshot, RepoCondition, SaveId,
    classify,
};
pub use names::new_app_branch_name;
pub use project::{
    GITIGNORE_TEMPLATE, OpenOutcome, Project, open_project, snapshot, start_saving_folder,
};
pub use recovery::{
    LOAD_ASIDE_PREFIX, RECOVERY_MAX_AGE_DAYS, RECOVERY_PREFIX, RecoveryEntry, Scope, list_recovery,
    prune_recovery, restore_recovery, throw_away,
};
pub use restore::{BringBackError, UnsavedPolicy, bring_back_everything, bring_back_file};
pub use safety::{
    Decision, Flag, FlagAction, FlagReason, JUNK_DIRS, allowed_actions, apply_decisions,
    find_secret_line, is_secret_name, scan,
};
pub use save::{
    AI_PATCH_CAP, NamingInput, SaveError, commit_all, hooks_configured, identity_missing,
    naming_input, normalize_name, set_identity_if_missing,
};
pub use savestates_git::Blocker;
pub use timeline::{
    HISTORY_PAGE, SaveState, Side, changes_in_save, files_at_save, read_file, timeline_page,
};
pub use timelines::{
    APP_PREFIX, GraphSave, MAIN_LINE_KEY, Timeline, TimelinesView, is_empty_app_timeline,
    list_timelines, read_older_saves, read_timelines, sweep_empty_timelines,
};
