//! Thin, safe wrapper around the system `git` executable.
#![forbid(unsafe_code)]

mod runner;

pub use runner::{
    GitError, GitExe, MIN_VERSION, Repo, RunOpts, Version, locate, parse_version, run_in,
};
