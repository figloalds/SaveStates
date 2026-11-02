//! The check that runs before a Quick Save: junk folders, possible secrets and large files, and
//! what to do about each one the user is warned of (spec section 6.8.1).

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::{Regex, RegexSet};
use savestates_git::{blob, path_in_history, rm_cached};
use similar::{Algorithm, DiffTag, capture_diff_slices_deadline};

use crate::error::CoreError;
use crate::gitignore;
use crate::model::{Change, ChangeKind, FileContent, MAX_DIFF_BYTES, classify};
use crate::project::Project;

/// Dependency and build folders that do not belong in a save (the same ones the default
/// `.gitignore` leaves out).
pub const JUNK_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    "build",
    "out",
    ".next",
    "target",
    "__pycache__",
    ".venv",
    "venv",
];

/// File name endings that mark a secret (`.ppk`: a PuTTY private key).
const SECRET_SUFFIXES: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".keystore", ".ppk"];

/// The first and the last line of a private key.
const KEY_BEGIN: &str = r"-----BEGIN [A-Z ]*PRIVATE KEY-----";
const KEY_END: &str = r"-----END [A-Z ]*PRIVATE KEY-----";

/// Common token shapes: private-key headers, AWS, GitHub, Anthropic, OpenAI-style, Slack and
/// Google API keys.
const SECRET_PATTERNS: &[&str] = &[
    KEY_BEGIN,
    r"AKIA[0-9A-Z]{16}",
    r"ghp_[A-Za-z0-9]{36}",
    r"github_pat_[A-Za-z0-9_]{22,}",
    r"sk-ant-[A-Za-z0-9_\-]{20,}",
    r"sk-[A-Za-z0-9]{20,}",
    r"xox[bp]-[A-Za-z0-9\-]{10,}",
    r"AIza[0-9A-Za-z_\-]{35}",
];

static SECRETS: LazyLock<RegexSet> = LazyLock::new(|| {
    RegexSet::new(SECRET_PATTERNS).expect("the secret patterns are valid regular expressions")
});

/// The same patterns as one expression, to find where each match is. None of them spans a
/// line break.
static SECRET_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&SECRET_PATTERNS.join("|"))
        .expect("the secret patterns are valid regular expressions")
});

static KEY_BEGIN_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(KEY_BEGIN).expect("the key pattern is a valid expression"));
static KEY_END_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(KEY_END).expect("the key pattern is a valid expression"));

/// What a secret is replaced with in text that leaves the computer.
const REDACTED: &str = "[redacted]";

/// How long the line diff of a changed file may run. Past that `similar` settles for a coarser,
/// still correct, answer that errs towards calling more lines new.
const DIFF_TIMEOUT: Duration = Duration::from_secs(1);

const BYTES_PER_MB: u64 = 1024 * 1024;

/// Why a file or folder is flagged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlagReason {
    /// The file's name looks like a secret's (`.env`, `*.pem`, ...).
    SecretName,
    /// A line of the file, 1-based, looks like a token or a private key.
    SecretContent { line: u32 },
    /// A dependency or build folder; the flag's path is the folder.
    JunkFolder,
    /// The file is bigger than the limit.
    Large { bytes: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flag {
    /// The file, or for [`FlagReason::JunkFolder`] the folder, relative to the project.
    pub path: String,
    pub reason: FlagReason,
    /// An earlier save already holds this file (or, for a folder, something in it).
    pub already_saved: bool,
    /// Some save in the project's history changed this path, even if it is gone now.
    pub in_older_saves: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlagAction {
    /// For a path not saved before: add it to `.gitignore` and unstage it, so this save leaves
    /// it out.
    LeaveOut,
    /// For a path saved before: the same, so the save records it as no longer saved (the files
    /// stay on disk).
    StopSaving,
    /// Close the warning so the user can fix the file.
    CancelAndFix,
    /// Save it as it is.
    SaveAnyway,
}

/// What the user may do about `flag`, the recommended action first.
pub fn allowed_actions(flag: &Flag) -> Vec<FlagAction> {
    match (&flag.reason, flag.already_saved) {
        // Ignoring a source file would be wrong, so a flagged line can only be fixed.
        (FlagReason::SecretContent { .. }, _) => {
            vec![FlagAction::CancelAndFix, FlagAction::SaveAnyway]
        }
        (_, false) => vec![FlagAction::LeaveOut, FlagAction::SaveAnyway],
        (_, true) => vec![FlagAction::StopSaving, FlagAction::SaveAnyway],
    }
}

/// True when a file called `file_name` (no folders), whatever its letter case, is likely a secret.
pub fn is_secret_name(file_name: &str) -> bool {
    let name = file_name.to_ascii_lowercase();
    if name == ".env" {
        return true;
    }
    if let Some(variant) = name.strip_prefix(".env.") {
        return variant != "example";
    }
    SECRET_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
        || name.starts_with("id_rsa")
        || name.starts_with("id_ed25519")
        || name
            .strip_prefix("credentials")
            .is_some_and(|rest| rest.ends_with(".json"))
}

/// The 1-based number of the first line of `text` that looks like a token or a private key.
pub fn find_secret_line(text: &str) -> Option<u32> {
    secret_lines(text).next().map(line_number)
}

/// `patch` (a diff) with its secrets replaced by `[redacted]`, on every line: added, removed
/// and unchanged ones alike.
///
/// * Every line of a private key, from its BEGIN line through its END line, becomes
///   `[redacted]` whole (after the diff's `+`, `-` or space). A hunk can start or end inside a
///   key, when it shows only the unchanged lines around a change: then everything from the
///   hunk's start to an END line, or from a BEGIN line to the hunk's end, is part of it.
/// * A hunk header loses the text git repeats after it (the nearest line before the hunk, which
///   can be a line of a key).
/// * On the other lines, whatever looks like a token is replaced.
pub(crate) fn redact_secrets(patch: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut in_key = false;
    // Where the lines of the current hunk start in `lines`.
    let mut hunk_start = 0;
    for line in patch.split_inclusive('\n') {
        if line.starts_with("diff --git ") || line.starts_with("@@") {
            in_key = false;
            lines.push(
                SECRET_TEXT
                    .replace_all(&hunk_range(line), REDACTED)
                    .into_owned(),
            );
            hunk_start = lines.len();
            continue;
        }
        if let Some(begin) = KEY_BEGIN_LINE.find_iter(line).last() {
            // A key that also ends on this line ends here.
            in_key = !KEY_END_LINE.is_match(&line[begin.end()..]);
        } else if in_key {
            in_key = !KEY_END_LINE.is_match(line);
        } else if KEY_END_LINE.is_match(line) {
            // The end of a key that began before this hunk: what the hunk showed of it so far
            // is part of it too.
            for earlier in &mut lines[hunk_start..] {
                *earlier = blanked(earlier);
            }
        } else {
            lines.push(SECRET_TEXT.replace_all(line, REDACTED).into_owned());
            continue;
        }
        lines.push(blanked(line));
    }
    lines.concat()
}

/// A hunk header (`@@ -4,4 +4,4 @@ text`) without the text after its second `@@`, keeping the
/// line break; any other line as it is.
fn hunk_range(line: &str) -> Cow<'_, str> {
    let end = line
        .strip_prefix("@@")
        .and_then(|rest| rest.find("@@"))
        .map(|at| at + 4);
    match end {
        Some(end) if end < line.trim_end_matches('\n').len() => {
            let newline = if line.ends_with('\n') { "\n" } else { "" };
            Cow::Owned(format!("{}{newline}", &line[..end]))
        }
        _ => Cow::Borrowed(line),
    }
}

/// A line of a private key: `[redacted]`, after the diff's `+`, `-` or space, and before the
/// line break.
fn blanked(line: &str) -> String {
    let marker = line
        .chars()
        .next()
        .filter(|c| matches!(c, '+' | '-' | ' '))
        .map_or_else(String::new, String::from);
    let newline = if line.ends_with('\n') { "\n" } else { "" };
    format!("{marker}{REDACTED}{newline}")
}

/// The 0-based indexes of the lines of `text` that look like a token or a private key.
fn secret_lines(text: &str) -> impl Iterator<Item = usize> + '_ {
    text.lines()
        .enumerate()
        .filter(|(_, line)| SECRETS.is_match(line))
        .map(|(index, _)| index)
}

fn line_number(index: usize) -> u32 {
    u32::try_from(index + 1).unwrap_or(u32::MAX)
}

/// The shallowest folder in `path` that is a junk folder, as a prefix of `path` without a
/// trailing slash. The last part of `path` is a file name, never a folder.
fn junk_folder(path: &str) -> Option<&str> {
    let mut start = 0;
    // Each part followed by a slash is a folder; the part after the last slash is the file.
    for (slash, _) in path.match_indices('/') {
        if JUNK_DIRS.contains(&&path[start..slash]) {
            return Some(&path[..slash]);
        }
        start = slash + 1;
    }
    None
}

/// A junk folder and what the changes under it say about it.
struct JunkGroup<'a> {
    path: &'a str,
    /// Something under it is not a deletion, so a save would put something there.
    has_content: bool,
    /// Something under it is not new, so an earlier save already holds part of it.
    has_saved: bool,
}

/// Checks the files `changes` would put into a save.
///
/// * Everything under a junk folder is one flag for that folder, with no per-file flags.
/// * Any other file gets at most one flag: its name first, then a token in what it gained, then
///   its size. Deleted files are never flagged, as there is nothing to save.
///
/// Junk folders come first, then files in the order of `changes`. `large_file_mb` is the size
/// above which a file counts as large.
pub fn scan(p: &Project, changes: &[Change], large_file_mb: u64) -> Result<Vec<Flag>, CoreError> {
    let limit = large_file_mb.saturating_mul(BYTES_PER_MB);

    let mut groups: Vec<JunkGroup> = Vec::new();
    let mut group_of: HashMap<&str, usize> = HashMap::new();
    let mut files = Vec::new();
    for change in changes {
        let deleted = change.kind == ChangeKind::Deleted;
        let saved = change.kind != ChangeKind::New;
        if let Some(folder) = junk_folder(&change.path) {
            let slot = *group_of.entry(folder).or_insert_with(|| {
                groups.push(JunkGroup {
                    path: folder,
                    has_content: false,
                    has_saved: false,
                });
                groups.len() - 1
            });
            groups[slot].has_content |= !deleted;
            groups[slot].has_saved |= saved;
        } else if !deleted && let Some(reason) = file_reason(p, change, limit)? {
            files.push(Flag {
                path: change.path.clone(),
                reason,
                already_saved: saved,
                in_older_saves: false,
            });
        }
    }

    let folders = groups
        .into_iter()
        .filter(|group| group.has_content)
        .map(|group| Flag {
            path: group.path.to_owned(),
            reason: FlagReason::JunkFolder,
            already_saved: group.has_saved,
            in_older_saves: false,
        });
    let mut flags: Vec<Flag> = folders.chain(files).collect();
    for flag in &mut flags {
        flag.in_older_saves = path_in_history(p.repo(), &flag.path)?;
    }
    Ok(flags)
}

/// The flag for one file outside the junk folders, if it deserves one.
fn file_reason(p: &Project, change: &Change, limit: u64) -> Result<Option<FlagReason>, CoreError> {
    let file_name = change.path.rsplit('/').next().unwrap_or(&change.path);
    if is_secret_name(file_name) {
        return Ok(Some(FlagReason::SecretName));
    }

    let full = p.root().join(&change.path);
    let bytes = match fs::symlink_metadata(&full) {
        Ok(meta) if meta.is_file() => meta.len(),
        // A folder, a link (saved as the link itself) or something already gone.
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if bytes <= MAX_DIFF_BYTES
        && let Some(line) = added_secret_line(p, change, &full)?
    {
        return Ok(Some(FlagReason::SecretContent { line }));
    }
    Ok((bytes > limit).then_some(FlagReason::Large { bytes }))
}

/// The line number, in the file on disk, of the first token-like line that this change adds.
/// Lines the last save already had do not count. `None` for a binary file.
fn added_secret_line(p: &Project, change: &Change, full: &Path) -> Result<Option<u32>, CoreError> {
    let bytes = match fs::read(full) {
        Ok(bytes) => bytes,
        // Gone since the size check.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let FileContent::Text(text) = classify(bytes) else {
        return Ok(None);
    };
    // Most files have no token at all, which settles it without asking git for the old text.
    let hits: Vec<usize> = secret_lines(&text).collect();
    if hits.is_empty() {
        return Ok(None);
    }

    let old_path = match &change.kind {
        ChangeKind::Changed => &change.path,
        ChangeKind::Renamed { from } => from,
        ChangeKind::New | ChangeKind::Deleted => {
            return Ok(hits.first().copied().map(line_number));
        }
    };
    let old = match blob(p.repo(), "HEAD", old_path)?.map(classify) {
        Some(FileContent::Text(old)) => old,
        _ => String::new(),
    };
    let is_added = added_lines(&old, &text);
    Ok(hits.into_iter().find(|&hit| is_added[hit]).map(line_number))
}

/// One flag per line of `new`: true when `old` does not have that line there. Line endings are
/// not part of a line, so a file switching between LF and CRLF changes nothing.
fn added_lines(old: &str, new: &str) -> Vec<bool> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let deadline = Instant::now().checked_add(DIFF_TIMEOUT);
    let mut added = vec![false; new_lines.len()];
    for op in capture_diff_slices_deadline(Algorithm::Myers, &old_lines, &new_lines, deadline) {
        if matches!(op.tag(), DiffTag::Insert | DiffTag::Replace) {
            for line in op.new_range() {
                added[line] = true;
            }
        }
    }
    added
}

/// The outcome of applying the user's choices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Go on with the save. `excluded` lists the paths that were left out or stopped.
    Proceed { excluded: Vec<String> },
    /// Do not save: show this file so the user can fix it.
    CancelAndFix { path: String },
}

/// Carries out the user's choice for each flag, in order.
///
/// The first [`FlagAction::CancelAndFix`] wins: it is returned at once, before anything else is
/// applied, so cancelling never changes the project. Otherwise [`FlagAction::LeaveOut`] and
/// [`FlagAction::StopSaving`] both add the path to `.gitignore` and take it out of the index,
/// so the next save leaves it out whether or not it was staged or saved before (the files stay
/// on disk), and [`FlagAction::SaveAnyway`] does nothing. Actions are applied as given:
/// offering only [`allowed_actions`] is up to the caller.
pub fn apply_decisions(
    p: &Project,
    decisions: &[(Flag, FlagAction)],
) -> Result<Decision, CoreError> {
    if let Some((flag, _)) = decisions
        .iter()
        .find(|(_, action)| *action == FlagAction::CancelAndFix)
    {
        return Ok(Decision::CancelAndFix {
            path: flag.path.clone(),
        });
    }

    let mut excluded: Vec<String> = Vec::new();
    for (flag, action) in decisions {
        if !matches!(action, FlagAction::LeaveOut | FlagAction::StopSaving) {
            continue;
        }
        gitignore::append_lines(p.root(), &[ignore_line(flag)])?;
        // An ignore line does not affect what is already in the index. That includes files a
        // failed save left staged, so `git add -A` would still save them. Both actions
        // therefore take the path out of the index too; for an untracked path this does nothing.
        let is_folder = flag.reason == FlagReason::JunkFolder;
        rm_cached(p.repo(), &flag.path, is_folder)?;
        if !excluded.contains(&flag.path) {
            excluded.push(flag.path.clone());
        }
    }
    Ok(Decision::Proceed { excluded })
}

/// The `.gitignore` line that leaves `flag` out: a junk folder by name, wherever it appears,
/// and a file by its exact path.
fn ignore_line(flag: &Flag) -> String {
    if flag.reason == FlagReason::JunkFolder {
        let name = flag
            .path
            .rsplit_once('/')
            .map_or(flag.path.as_str(), |(_, name)| name);
        return format!("{name}/");
    }
    let mut line = String::with_capacity(flag.path.len() + 1);
    line.push('/');
    for c in flag.path.chars() {
        // Characters that would make git read the path as a pattern.
        if matches!(c, '\\' | '*' | '?' | '[') {
            line.push('\\');
        }
        line.push(c);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_of_token_is_redacted_and_the_rest_kept() {
        let text = concat!(
            "@@ -1,4 +1,4 @@ const ant = sk-ant-api03-AbCdEfGhIjKlMnOp_Qr-StUv\n",
            "+aws = AKIAABCDEFGHIJKLMNOP; gh = ghp_abcdefghijklmnopqrstuvwxyzABCDEFGHIJ\n",
            " pat = github_pat_0123456789abcdefghijKL\n",
            "-ant = sk-ant-api03-AbCdEfGhIjKlMnOp_Qr-StUv; oai = sk-AbCdEfGhIjKlMnOpQrStUv\n",
            "slack = xoxb-1234567890-abc; google = AIzaSyA1234567890abcdefghijklmnopqrstuv\n",
            "nothing secret here",
        );
        assert_eq!(
            redact_secrets(text),
            concat!(
                // What git repeats after a hunk header is dropped.
                "@@ -1,4 +1,4 @@\n",
                "+aws = [redacted]; gh = [redacted]\n",
                " pat = [redacted]\n",
                "-ant = [redacted]; oai = [redacted]\n",
                "slack = [redacted]; google = [redacted]\n",
                "nothing secret here",
            )
        );
        assert_eq!(redact_secrets(""), "");
    }

    #[test]
    fn a_private_key_is_blanked_from_its_first_line_to_its_last() {
        let patch = concat!(
            "diff --git a/k.js b/k.js\n",
            "@@ -1,6 +1,6 @@\n",
            " const PEM = `\n",
            "------BEGIN RSA PRIVATE KEY-----\n",
            "-MIIEowIBAAKCAQEA\n",
            "------END RSA PRIVATE KEY-----\n",
            "+-----BEGIN EC PRIVATE KEY-----\n",
            "+MHcCAQEEIA\n",
            "+-----END EC PRIVATE KEY-----\n",
            " `;\n",
            "+const k = \"-----BEGIN PRIVATE KEY-----\\nMIIE\\n-----END PRIVATE KEY-----\";\n",
            " const porta = 80;\n",
        );
        assert_eq!(
            redact_secrets(patch),
            concat!(
                "diff --git a/k.js b/k.js\n",
                "@@ -1,6 +1,6 @@\n",
                " const PEM = `\n",
                "-[redacted]\n",
                "-[redacted]\n",
                "-[redacted]\n",
                "+[redacted]\n",
                "+[redacted]\n",
                "+[redacted]\n",
                " `;\n",
                // A key written on one line ends on it.
                "+[redacted]\n",
                " const porta = 80;\n",
            )
        );
    }

    #[test]
    fn a_hunk_that_starts_or_ends_inside_a_key_blanks_that_part() {
        let patch = concat!(
            "diff --git a/after.js b/after.js\n",
            "@@ -4,4 +4,4 @@ MIIEowIBAAKCAQEA\n",
            " QkM9mW3pZ\n",
            " 0nLVkjRo9z\n",
            " -----END PRIVATE KEY-----`;\n",
            "-const porta = 80;\n",
            "+const porta = 81;\n",
            "diff --git a/before.js b/before.js\n",
            "@@ -1,4 +1,4 @@\n",
            "-const porta = 80;\n",
            "+const porta = 81;\n",
            " const PEM = `-----BEGIN PRIVATE KEY-----\n",
            " MIIEowIBAA\n",
            "@@ -40,2 +40,2 @@\n",
            "-old\n",
            "+new\n",
        );
        assert_eq!(
            redact_secrets(patch),
            concat!(
                "diff --git a/after.js b/after.js\n",
                "@@ -4,4 +4,4 @@\n",
                " [redacted]\n",
                " [redacted]\n",
                " [redacted]\n",
                "-const porta = 80;\n",
                "+const porta = 81;\n",
                "diff --git a/before.js b/before.js\n",
                "@@ -1,4 +1,4 @@\n",
                "-const porta = 80;\n",
                "+const porta = 81;\n",
                " [redacted]\n",
                " [redacted]\n",
                // The next hunk is outside the key.
                "@@ -40,2 +40,2 @@\n",
                "-old\n",
                "+new\n",
            )
        );
    }
}
