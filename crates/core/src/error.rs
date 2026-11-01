//! Errors shared by the core operations.

use savestates_git::GitError;

#[derive(thiserror::Error, Debug)]
pub enum CoreError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("there are no saves yet")]
    NoSavesYet,
    #[error("recovering would overwrite changes that cannot be set aside")]
    RecoveryConflict,
    #[error("not found: {0}")]
    NotFound(String),
}
