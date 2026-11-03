//! Thin, safe wrapper around the system `git` executable.
#![forbid(unsafe_code)]

mod branches;
mod content;
mod log;
mod runner;
mod stash;
mod state;
mod status;
mod write;

pub use branches::{
    BranchInfo, create_branch, delete_branch, first_parent_count, head_branch, head_commit,
    list_branches, on_first_parent_line, own_first_parent_saves, switch_new, switch_to,
};
pub use content::{blob, blob_size, ls_tree_names};
pub use log::{LogEntry, first_parent, log_first_parent, log_graph, name_status};
pub use runner::{
    GitError, GitExe, MIN_VERSION, Repo, RunOpts, Version, locate, parse_version, run_in,
};
pub use stash::{
    delete_ref, refs_with_prefix, restore_worktree, snapshot_files_to_ref, stash_apply,
    stash_snapshot_paths_to_ref, stash_snapshot_to_ref, update_ref,
};
pub use state::{Blocker, blockers, other_branches_exist};
pub use status::{EntryKind, Status, StatusEntry, parse_status_v2, status};
pub use write::{
    add_all, append_info_exclude, commit, config_get, config_get_global, config_get_regexp,
    config_remove_section, config_set, config_set_global, config_unset, diff_cached, git_path,
    hooks_configured, ignored_entries, ignored_entries_by_rule, init, is_bare, path_in_history,
    rm_cached, toplevel,
};
