//! What a revision's tree contains: its file names and the bytes of its files.

use crate::runner::{GitError, Repo, RunOpts};

/// Every file in the tree of `rev`, with its full path, in git's order.
pub fn ls_tree_names(repo: &Repo, rev: &str) -> Result<Vec<String>, GitError> {
    let out = repo.run(
        &["ls-tree", "-r", "-z", "--name-only", rev],
        &RunOpts::read_only(),
    )?;
    Ok(out
        .split(|&b| b == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect())
}

/// The raw bytes stored for `path` at `rev` (no text conversion or filters), or `None` when
/// the tree has nothing at that path.
pub fn blob(repo: &Repo, rev: &str, path: &str) -> Result<Option<Vec<u8>>, GitError> {
    let listed = repo.run(&["ls-tree", "-z", rev, "--", path], &RunOpts::read_only())?;
    if listed.is_empty() {
        return Ok(None);
    }
    let object = format!("{rev}:{path}");
    repo.run(&["cat-file", "blob", &object], &RunOpts::read_only())
        .map(Some)
}

/// The size in bytes of the file at `path` in `rev`, without reading it. `None` when the tree
/// has no file there (nothing at the path, or a folder or submodule).
pub fn blob_size(repo: &Repo, rev: &str, path: &str) -> Result<Option<u64>, GitError> {
    let listed = repo.run(
        &["ls-tree", "-l", "-z", rev, "--", path],
        &RunOpts::read_only(),
    )?;
    Ok(parse_blob_size(&listed))
}

/// Parses the first `<mode> SP <type> SP <oid> SP <size> TAB <path> NUL` record.
fn parse_blob_size(listed: &[u8]) -> Option<u64> {
    let meta = listed.split(|&b| b == b'\t').next()?;
    let meta = String::from_utf8_lossy(meta);
    let mut fields = meta.split_whitespace();
    let kind = fields.nth(1)?;
    let size = fields.nth(1)?;
    if kind == "blob" {
        size.parse().ok()
    } else {
        None
    }
}
