//! Opening a folder as a project, starting to save a plain folder, and reading a project's state.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use savestates_git::{
    Blocker, EntryKind, GitError, GitExe, Repo, StatusEntry, blockers, init, is_bare,
    other_branches_exist, status, toplevel,
};

use crate::error::CoreError;
use crate::model::{Change, ChangeKind, ProjectSnapshot, RepoCondition, SaveId};

/// Written to a new project's `.gitignore` when it has none (spec section 6.7.1).
pub const GITIGNORE_TEMPLATE: &str = concat!(
    "# OS\n",
    ".DS_Store\n",
    "Thumbs.db\n",
    "desktop.ini\n",
    "# Dependencies and build output\n",
    "node_modules/\n",
    "dist/\n",
    "build/\n",
    "out/\n",
    ".next/\n",
    "target/\n",
    "__pycache__/\n",
    ".venv/\n",
    "venv/\n",
    "# Secrets\n",
    ".env\n",
    ".env.*\n",
    "!.env.example\n",
    "# Editors and logs\n",
    "*.log\n",
    ".idea/\n",
    ".vs/\n",
);

/// A folder under version control, ready to be read and saved to.
#[derive(Clone, Debug)]
pub struct Project {
    repo: Repo,
    bare: bool,
}

impl Project {
    pub fn root(&self) -> &Path {
        &self.repo.root
    }

    pub fn repo(&self) -> &Repo {
        &self.repo
    }
}

#[derive(Debug)]
pub enum OpenOutcome {
    Opened(Project),
    /// The folder exists but is not inside a repository.
    NotARepo,
    /// The path does not exist or is not a folder.
    Missing,
}

/// Opens `folder`, or any folder inside a work tree, as a project.
pub fn open_project(git: &GitExe, folder: &Path) -> Result<OpenOutcome, CoreError> {
    match fs::metadata(folder) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Ok(OpenOutcome::Missing),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(OpenOutcome::Missing),
        Err(e) => return Err(e.into()),
    }

    let bare = match is_bare(git, folder) {
        Ok(bare) => bare,
        Err(GitError::NotARepo) => return Ok(OpenOutcome::NotARepo),
        Err(e) => return Err(e.into()),
    };
    // A bare repository has no work tree, so the folder itself stands for the project.
    let root = if bare {
        dunce::canonicalize(folder)?
    } else {
        match toplevel(git, folder) {
            Ok(root) => root,
            Err(GitError::NotARepo) => return Ok(OpenOutcome::NotARepo),
            Err(e) => return Err(e.into()),
        }
    };
    Ok(OpenOutcome::Opened(Project {
        repo: Repo {
            exe: git.clone(),
            root,
        },
        bare,
    }))
}

/// Turns a plain folder into a project: `git init` on `main`, plus a default `.gitignore`
/// unless the folder already has one.
pub fn start_saving_folder(git: &GitExe, folder: &Path) -> Result<Project, CoreError> {
    init(git, folder)?;

    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(folder.join(".gitignore"))
    {
        Ok(mut file) => file.write_all(GITIGNORE_TEMPLATE.as_bytes())?,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }

    Ok(Project {
        repo: Repo {
            exe: git.clone(),
            root: toplevel(git, folder)?,
        },
        bare: false,
    })
}

/// Reads the project's current state. Never fails: a git failure becomes [`RepoCondition::Broken`].
pub fn snapshot(p: &Project) -> ProjectSnapshot {
    read_snapshot(p).unwrap_or_else(|e| ProjectSnapshot {
        condition: RepoCondition::Broken {
            details: e.to_string(),
        },
        timeline_name: None,
        head: None,
        changes: Vec::new(),
        other_timelines: false,
    })
}

fn read_snapshot(p: &Project) -> Result<ProjectSnapshot, GitError> {
    if p.bare {
        return Ok(ProjectSnapshot {
            condition: RepoCondition::ReadOnly(vec![Blocker::Bare]),
            timeline_name: None,
            head: None,
            changes: Vec::new(),
            other_timelines: false,
        });
    }

    let status = status(&p.repo)?;
    let blockers = blockers(&p.repo, &status)?;
    let other_timelines = other_branches_exist(&p.repo)?;
    Ok(ProjectSnapshot {
        condition: if blockers.is_empty() {
            RepoCondition::Normal
        } else {
            RepoCondition::ReadOnly(blockers)
        },
        timeline_name: status.branch,
        head: status.head_oid.map(SaveId),
        changes: status.entries.into_iter().map(into_change).collect(),
        other_timelines,
    })
}

fn into_change(entry: StatusEntry) -> Change {
    let kind = match (entry.kind, entry.orig_path) {
        (EntryKind::Renamed, Some(from)) => ChangeKind::Renamed { from },
        (EntryKind::Added | EntryKind::Untracked, _) => ChangeKind::New,
        (EntryKind::Deleted, _) => ChangeKind::Deleted,
        // A conflicted file is still a file the user has changed.
        (
            EntryKind::Modified | EntryKind::TypeChanged | EntryKind::Unmerged | EntryKind::Renamed,
            _,
        ) => ChangeKind::Changed,
    };
    Change {
        path: entry.path,
        kind,
    }
}
