//! Timelines (git branches) and the saves drawn on them (spec §3, §6).

use std::path::PathBuf;

use crate::model::SaveId;
use crate::timeline::SaveState;

/// The prefix of every branch the app makes for itself (`savestates/<words>`, spec §5.1).
pub const APP_PREFIX: &str = "savestates/";

/// One local branch, seen as a timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timeline {
    /// The git branch name, e.g. `main` or `savestates/tarde-de-domingo`.
    pub branch: String,
    /// The branch's last save.
    pub tip: SaveId,
    /// The name the user gave it (`branch.<name>.description`), if any.
    pub display_name: Option<String>,
    pub archived: bool,
    /// Whether this is the project's main line (spec §3.2).
    pub main_line: bool,
    /// Whether the project folder is on this timeline.
    pub current: bool,
    /// The folder of another git worktree this branch is checked out in.
    pub elsewhere: Option<PathBuf>,
}

impl Timeline {
    /// Whether the app made this timeline (its git name starts with [`APP_PREFIX`]).
    pub fn app_made(&self) -> bool {
        self.branch.starts_with(APP_PREFIX)
    }
}

/// A save together with the saves it was made on top of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphSave {
    pub save: SaveState,
    /// The first parent is the save before it on its own line; more than one means a merge.
    pub parents: Vec<SaveId>,
}

/// Everything the timeline screen needs to draw every timeline at once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimelinesView {
    /// Every local branch, by name.
    pub timelines: Vec<Timeline>,
    /// The save the project folder is on; `None` before the first save.
    pub head: Option<SaveId>,
    /// Whether the project folder is on a save rather than on a timeline.
    pub detached: bool,
    /// The loaded saves of every branch, newest first, as `log_graph` lists them.
    pub saves: Vec<GraphSave>,
    /// Whether older saves exist beyond the loaded ones.
    pub more_available: bool,
}
