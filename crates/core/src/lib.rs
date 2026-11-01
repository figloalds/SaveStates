//! SaveStates domain model and the operations built on top of `savestates-git`.
#![forbid(unsafe_code)]

mod error;
mod model;
mod project;

pub use error::CoreError;
pub use model::{Change, ChangeKind, ProjectSnapshot, RepoCondition, SaveId};
pub use project::{
    GITIGNORE_TEMPLATE, OpenOutcome, Project, open_project, snapshot, start_saving_folder,
};
pub use savestates_git::Blocker;
