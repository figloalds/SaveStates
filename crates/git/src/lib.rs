//! Thin, safe wrapper around the system `git` executable.
#![forbid(unsafe_code)]

mod runner;
mod state;
mod status;
mod write;

pub use runner::{
    GitError, GitExe, MIN_VERSION, Repo, RunOpts, Version, locate, parse_version, run_in,
};
pub use state::{Blocker, blockers, other_branches_exist};
pub use status::{EntryKind, Status, StatusEntry, parse_status_v2, status};
pub use write::{init, is_bare, toplevel};
