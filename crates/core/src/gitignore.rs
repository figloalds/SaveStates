//! Adding lines to a project's `.gitignore`.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

/// Appends each of `lines` to `root/.gitignore` unless the file already has that exact line
/// (or an earlier entry of `lines` is the same). Creates the file if it is missing, and starts
/// on a fresh line when the existing last line has no newline. Leaves the file alone when there
/// is nothing to add.
pub fn append_lines(root: &Path, lines: &[String]) -> io::Result<()> {
    let path = root.join(".gitignore");
    let existing = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };

    let text = String::from_utf8_lossy(&existing);
    let mut seen: HashSet<&str> = text.lines().collect();
    let mut addition = String::new();
    for line in lines {
        if seen.insert(line) {
            addition.push_str(line);
            addition.push('\n');
        }
    }
    if addition.is_empty() {
        return Ok(());
    }
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        addition.insert(0, '\n');
    }

    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?
        .write_all(addition.as_bytes())
}
