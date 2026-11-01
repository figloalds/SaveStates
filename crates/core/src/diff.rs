//! The side-by-side comparison of one file at two points in time.
//!
//! [`side_by_side`] turns two [`FileContent`]s into rows of "before" and "after" lines that the
//! comparison view paints directly: matching lines side by side, changed lines paired up with
//! the changed words marked, and long unchanged stretches folded away.

use std::borrow::Cow;

use similar::{ChangeTag, DiffOp, TextDiff};

use crate::model::{FileContent, MAX_DIFF_BYTES};

/// How many unchanged rows stay visible on each side of a change.
pub const CONTEXT_LINES: usize = 3;

/// More lines than this on either side is "too large" to compare unless the user asks for it.
pub const MAX_DIFF_LINES: usize = 5_000;

/// Unchanged runs shorter than this are not worth folding away.
const MIN_COLLAPSED_ROWS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// The same line on both sides.
    Same,
    /// Only on the "before" side.
    Removed,
    /// Only on the "after" side.
    Added,
    /// A "before" line and the "after" line that replaced it.
    Changed,
}

/// A piece of a line; `emphasized` marks the words that differ on a [`RowKind::Changed`] row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seg {
    pub text: String,
    pub emphasized: bool,
}

/// One line on one side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    /// 1-based line number on this side.
    pub number: u32,
    /// The line without its trailing newline, split into [`Seg`]s.
    pub segs: Vec<Seg>,
}

/// One row of the comparison. `left` is absent on `Added` rows, `right` on `Removed` rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffRow {
    pub kind: RowKind,
    pub left: Option<DiffLine>,
    pub right: Option<DiffLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffBlock {
    /// Rows to paint.
    Shown(Vec<DiffRow>),
    /// Unchanged rows folded away; the user can expand them.
    Collapsed(Vec<DiffRow>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffView {
    Blocks(Vec<DiffBlock>),
    Identical,
    /// The texts differ only in `\r\n` versus `\n`.
    OnlyLineEndings,
    Binary,
    /// Too big to compare unless forced. `lines` counts the larger side; it is `0` for a side
    /// that was never read.
    TooLarge {
        lines: usize,
        bytes: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffResult {
    pub view: DiffView,
    pub before_missing: bool,
    pub after_missing: bool,
}

/// Compares `before` with `after`.
///
/// A [`FileContent::Missing`] side counts as an empty file and is flagged in the result. Lines
/// are compared with `\r\n` read as `\n` (see [`DiffView::OnlyLineEndings`]), so a working copy
/// checked out with CRLF line endings does not show every line as changed. Unless `force` is
/// set, a side with more than [`MAX_DIFF_LINES`] lines or [`MAX_DIFF_BYTES`] bytes is not
/// compared.
pub fn side_by_side(before: &FileContent, after: &FileContent, force: bool) -> DiffResult {
    DiffResult {
        view: view(before, after, force),
        before_missing: *before == FileContent::Missing,
        after_missing: *after == FileContent::Missing,
    }
}

fn view(before: &FileContent, after: &FileContent, force: bool) -> DiffView {
    if *before == FileContent::Binary || *after == FileContent::Binary {
        return DiffView::Binary;
    }
    let (Some(old), Some(new)) = (readable_text(before), readable_text(after)) else {
        // One side was never read, so there is nothing to compare.
        return too_large(before, after);
    };

    if old == new {
        return DiffView::Identical;
    }
    let (old, new) = (unify_line_endings(old), unify_line_endings(new));
    if old == new {
        return DiffView::OnlyLineEndings;
    }
    if !force && (exceeds_limits(before) || exceeds_limits(after)) {
        return too_large(before, after);
    }
    DiffView::Blocks(collapse(build_rows(&old, &new)))
}

/// The text of a side that is available: `Missing` is empty. `None` for `Binary` and `TooLarge`.
fn readable_text(content: &FileContent) -> Option<&str> {
    match content {
        FileContent::Missing => Some(""),
        FileContent::Text(text) => Some(text),
        FileContent::Binary | FileContent::TooLarge { .. } => None,
    }
}

fn unify_line_endings(text: &str) -> Cow<'_, str> {
    if text.contains("\r\n") {
        Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(text)
    }
}

/// Lines, each with its trailing `\n`. Only `\n` ends a line, as in git.
fn split_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn line_count(content: &FileContent) -> usize {
    readable_text(content).map_or(0, |text| text.split_inclusive('\n').count())
}

fn byte_count(content: &FileContent) -> u64 {
    match content {
        FileContent::Text(text) => text.len() as u64,
        FileContent::TooLarge { bytes } => *bytes,
        FileContent::Missing | FileContent::Binary => 0,
    }
}

fn exceeds_limits(content: &FileContent) -> bool {
    line_count(content) > MAX_DIFF_LINES || byte_count(content) > MAX_DIFF_BYTES
}

fn too_large(before: &FileContent, after: &FileContent) -> DiffView {
    DiffView::TooLarge {
        lines: line_count(before).max(line_count(after)),
        bytes: byte_count(before).max(byte_count(after)),
    }
}

fn line_number(index: usize) -> u32 {
    u32::try_from(index + 1).unwrap_or(u32::MAX)
}

fn without_newline(line: &str) -> &str {
    line.strip_suffix('\n').unwrap_or(line)
}

fn plain_line(lines: &[&str], index: usize) -> DiffLine {
    DiffLine {
        number: line_number(index),
        segs: vec![Seg {
            text: without_newline(lines[index]).to_owned(),
            emphasized: false,
        }],
    }
}

/// Appends to `segs`, merging into the last segment when the emphasis is the same.
fn push_seg(segs: &mut Vec<Seg>, text: &str, emphasized: bool) {
    match segs.last_mut() {
        Some(last) if last.emphasized == emphasized => last.text.push_str(text),
        _ => segs.push(Seg {
            text: text.to_owned(),
            emphasized,
        }),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Space,
    Word,
    Other,
}

fn char_class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Other
    }
}

/// Splits a line into runs of whitespace, runs of word characters and single other characters,
/// so `24;` becomes `24` and `;`. (`TextDiff::from_words` splits on whitespace only and would
/// mark the whole of `24;`.)
fn word_tokens(line: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut previous: Option<CharClass> = None;
    for (index, c) in line.char_indices() {
        let class = char_class(c);
        if previous.is_some_and(|p| p != class || class == CharClass::Other) {
            tokens.push(&line[start..index]);
            start = index;
        }
        previous = Some(class);
    }
    if start < line.len() {
        tokens.push(&line[start..]);
    }
    tokens
}

/// The segments of a replaced line and of its replacement, with the words that differ
/// emphasized.
fn emphasized_segs(old: &str, new: &str) -> (Vec<Seg>, Vec<Seg>) {
    let (old_words, new_words) = (word_tokens(old), word_tokens(new));
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for change in TextDiff::from_slices(&old_words, &new_words).iter_all_changes() {
        let word = change.value();
        match change.tag() {
            ChangeTag::Equal => {
                push_seg(&mut left, word, false);
                push_seg(&mut right, word, false);
            }
            ChangeTag::Delete => push_seg(&mut left, word, true),
            ChangeTag::Insert => push_seg(&mut right, word, true),
        }
    }
    // An empty line has no words at all; it still gets its one (empty) segment.
    for segs in [&mut left, &mut right] {
        if segs.is_empty() {
            push_seg(segs, "", false);
        }
    }
    (left, right)
}

fn changed_row(old_lines: &[&str], old: usize, new_lines: &[&str], new: usize) -> DiffRow {
    let (left, right) = emphasized_segs(
        without_newline(old_lines[old]),
        without_newline(new_lines[new]),
    );
    DiffRow {
        kind: RowKind::Changed,
        left: Some(DiffLine {
            number: line_number(old),
            segs: left,
        }),
        right: Some(DiffLine {
            number: line_number(new),
            segs: right,
        }),
    }
}

fn build_rows(old: &str, new: &str) -> Vec<DiffRow> {
    let (old_lines, new_lines) = (split_lines(old), split_lines(new));
    let diff = TextDiff::from_slices(&old_lines, &new_lines);

    let removed = |index: usize| DiffRow {
        kind: RowKind::Removed,
        left: Some(plain_line(&old_lines, index)),
        right: None,
    };
    let added = |index: usize| DiffRow {
        kind: RowKind::Added,
        left: None,
        right: Some(plain_line(&new_lines, index)),
    };

    let mut rows = Vec::new();
    for op in diff.ops() {
        match *op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => rows.extend((0..len).map(|k| DiffRow {
                kind: RowKind::Same,
                left: Some(plain_line(&old_lines, old_index + k)),
                right: Some(plain_line(&new_lines, new_index + k)),
            })),
            DiffOp::Delete {
                old_index, old_len, ..
            } => rows.extend((old_index..old_index + old_len).map(removed)),
            DiffOp::Insert {
                new_index, new_len, ..
            } => rows.extend((new_index..new_index + new_len).map(added)),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let paired = old_len.min(new_len);
                rows.extend(
                    (0..paired)
                        .map(|k| changed_row(&old_lines, old_index + k, &new_lines, new_index + k)),
                );
                rows.extend((old_index + paired..old_index + old_len).map(removed));
                rows.extend((new_index + paired..new_index + new_len).map(added));
            }
        }
    }
    rows
}

/// Groups `rows` into blocks, folding away runs of at least [`MIN_COLLAPSED_ROWS`] unchanged
/// rows that are more than [`CONTEXT_LINES`] from any change.
fn collapse(rows: Vec<DiffRow>) -> Vec<DiffBlock> {
    let fold = rows_to_fold(&rows);
    let mut blocks: Vec<DiffBlock> = Vec::new();
    for (row, folded) in rows.into_iter().zip(fold) {
        match blocks.last_mut() {
            Some(DiffBlock::Shown(run)) if !folded => run.push(row),
            Some(DiffBlock::Collapsed(run)) if folded => run.push(row),
            _ if folded => blocks.push(DiffBlock::Collapsed(vec![row])),
            _ => blocks.push(DiffBlock::Shown(vec![row])),
        }
    }
    blocks
}

/// For each row, whether it belongs to a run to fold away.
fn rows_to_fold(rows: &[DiffRow]) -> Vec<bool> {
    let mut fold = far_from_changes(rows);
    let mut start = 0;
    while start < fold.len() {
        if !fold[start] {
            start += 1;
            continue;
        }
        let end = start + fold[start..].iter().take_while(|&&far| far).count();
        if end - start < MIN_COLLAPSED_ROWS {
            fold[start..end].fill(false);
        }
        start = end;
    }
    fold
}

/// For each row, whether it is `Same` and more than [`CONTEXT_LINES`] rows from every change.
fn far_from_changes(rows: &[DiffRow]) -> Vec<bool> {
    let mut distance = vec![usize::MAX; rows.len()];

    let mut since_change = usize::MAX;
    for (slot, row) in distance.iter_mut().zip(rows) {
        since_change = if row.kind == RowKind::Same {
            since_change.saturating_add(1)
        } else {
            0
        };
        *slot = since_change;
    }
    let mut until_change = usize::MAX;
    for (slot, row) in distance.iter_mut().zip(rows).rev() {
        until_change = if row.kind == RowKind::Same {
            until_change.saturating_add(1)
        } else {
            0
        };
        *slot = (*slot).min(until_change);
    }
    distance.into_iter().map(|d| d > CONTEXT_LINES).collect()
}
