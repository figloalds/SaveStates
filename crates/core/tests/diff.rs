use savestates_core::{
    CONTEXT_LINES, DiffBlock, DiffLine, DiffResult, DiffRow, DiffView, FileContent, MAX_DIFF_BYTES,
    RowKind, side_by_side,
};

fn text(s: &str) -> FileContent {
    FileContent::Text(s.to_owned())
}

/// `l1\nl2\n...\nl{count}\n`, with line `changed` (1-based) spelled `L{changed}` instead.
fn numbered_lines(count: usize, changed: Option<usize>) -> String {
    (1..=count)
        .map(|n| {
            if changed == Some(n) {
                format!("L{n}\n")
            } else {
                format!("l{n}\n")
            }
        })
        .collect()
}

fn blocks(result: &DiffResult) -> &[DiffBlock] {
    match &result.view {
        DiffView::Blocks(blocks) => blocks,
        other => panic!("expected Blocks, got {other:?}"),
    }
}

fn block_rows(block: &DiffBlock) -> &[DiffRow] {
    match block {
        DiffBlock::Shown(rows) | DiffBlock::Collapsed(rows) => rows,
    }
}

fn kinds(rows: &[DiffRow]) -> Vec<RowKind> {
    rows.iter().map(|r| r.kind).collect()
}

fn line_text(line: &DiffLine) -> String {
    line.segs.iter().map(|s| s.text.as_str()).collect()
}

fn emphasized(line: &DiffLine) -> Vec<&str> {
    line.segs
        .iter()
        .filter(|s| s.emphasized)
        .map(|s| s.text.as_str())
        .collect()
}

#[test]
fn identical() {
    let r = side_by_side(&text("a\nb\n"), &text("a\nb\n"), false);
    assert_eq!(r.view, DiffView::Identical);
    assert!(!r.before_missing);
    assert!(!r.after_missing);
}

#[test]
fn single_change() {
    let r = side_by_side(&text("a\nb\nc\n"), &text("a\nB\nc\n"), false);
    let blocks = blocks(&r);
    assert_eq!(blocks.len(), 1);
    let DiffBlock::Shown(rows) = &blocks[0] else {
        panic!("expected Shown, got {:?}", blocks[0]);
    };
    assert_eq!(
        kinds(rows),
        [RowKind::Same, RowKind::Changed, RowKind::Same]
    );

    let left = rows[1].left.as_ref().unwrap();
    let right = rows[1].right.as_ref().unwrap();
    assert_eq!(line_text(left), "b");
    assert_eq!(line_text(right), "B");
    assert!(left.segs.iter().all(|s| s.emphasized));
    assert!(right.segs.iter().all(|s| s.emphasized));
}

#[test]
fn collapses_far_context() {
    let r = side_by_side(
        &text(&numbered_lines(100, None)),
        &text(&numbered_lines(100, Some(50))),
        false,
    );
    let blocks = blocks(&r);
    assert_eq!(blocks.len(), 3);
    assert!(matches!(&blocks[0], DiffBlock::Collapsed(rows) if rows.len() == 46));
    let DiffBlock::Shown(middle) = &blocks[1] else {
        panic!("expected Shown, got {:?}", blocks[1]);
    };
    assert_eq!(middle.len(), 2 * CONTEXT_LINES + 1);
    assert_eq!(middle[CONTEXT_LINES].kind, RowKind::Changed);
    assert!(matches!(&blocks[2], DiffBlock::Collapsed(rows) if rows.len() == 47));

    // The collapsed rows are still real rows, and nothing is lost or reordered.
    let first = block_rows(&blocks[0]);
    assert_eq!(first[0].left.as_ref().unwrap().number, 1);
    assert_eq!(first[45].left.as_ref().unwrap().number, 46);
    let last = block_rows(&blocks[2]);
    assert_eq!(last[0].right.as_ref().unwrap().number, 54);
    assert_eq!(last[46].right.as_ref().unwrap().number, 100);
}

#[test]
fn short_runs_not_collapsed() {
    let r = side_by_side(
        &text(&numbered_lines(9, None)),
        &text(&numbered_lines(9, Some(5))),
        false,
    );
    let blocks = blocks(&r);
    assert_eq!(blocks.len(), 1);
    assert!(matches!(&blocks[0], DiffBlock::Shown(rows) if rows.len() == 9));
}

#[test]
fn collapses_between_two_changes_only_when_four_rows_hide() {
    // Two changed lines with `gap` Same rows between them; each keeps 3 rows of context.
    let two = |gap: usize| {
        let total = gap + 2 + 2;
        let before = numbered_lines(total, None);
        let after: String = (1..=total)
            .map(|n| {
                if n == 2 || n == 2 + gap + 1 {
                    format!("L{n}\n")
                } else {
                    format!("l{n}\n")
                }
            })
            .collect();
        side_by_side(&text(&before), &text(&after), false)
    };

    // gap 9: only 3 rows would be hidden, so everything stays in one Shown block.
    let r = two(9);
    assert_eq!(blocks(&r).len(), 1);
    // gap 10: 4 rows would be hidden, so they collapse.
    let r = two(10);
    let b = blocks(&r);
    assert_eq!(b.len(), 3);
    assert!(matches!(&b[1], DiffBlock::Collapsed(rows) if rows.len() == 4));
}

#[test]
fn new_file() {
    let r = side_by_side(&FileContent::Missing, &text("x\ny\n"), false);
    assert!(r.before_missing);
    assert!(!r.after_missing);
    let blocks = blocks(&r);
    assert_eq!(blocks.len(), 1);
    let rows = block_rows(&blocks[0]);
    assert_eq!(kinds(rows), [RowKind::Added, RowKind::Added]);
    assert!(rows.iter().all(|row| row.left.is_none()));
    assert_eq!(line_text(rows[0].right.as_ref().unwrap()), "x");
    assert_eq!(line_text(rows[1].right.as_ref().unwrap()), "y");
}

#[test]
fn deleted_file() {
    let r = side_by_side(&text("x\ny\n"), &FileContent::Missing, false);
    assert!(r.after_missing);
    assert!(!r.before_missing);
    let blocks = blocks(&r);
    assert_eq!(blocks.len(), 1);
    let rows = block_rows(&blocks[0]);
    assert_eq!(kinds(rows), [RowKind::Removed, RowKind::Removed]);
    assert!(rows.iter().all(|row| row.right.is_none()));
    assert_eq!(line_text(rows[0].left.as_ref().unwrap()), "x");
}

#[test]
fn missing_against_an_empty_file_is_identical() {
    let r = side_by_side(&FileContent::Missing, &text(""), false);
    assert_eq!(r.view, DiffView::Identical);
    assert!(r.before_missing);
    assert!(!r.after_missing);
}

#[test]
fn crlf_only() {
    let r = side_by_side(&text("a\r\nb\r\n"), &text("a\nb\n"), false);
    assert_eq!(r.view, DiffView::OnlyLineEndings);
}

#[test]
fn crlf_mixed_with_real_change() {
    let r = side_by_side(&text("a\r\nb\r\nc\r\n"), &text("a\nB\nc\n"), false);
    let blocks = blocks(&r);
    assert_eq!(blocks.len(), 1);
    let DiffBlock::Shown(rows) = &blocks[0] else {
        panic!("expected Shown, got {:?}", blocks[0]);
    };
    assert_eq!(
        kinds(rows),
        [RowKind::Same, RowKind::Changed, RowKind::Same]
    );
    // No carriage return leaks into what is shown.
    assert_eq!(line_text(rows[0].left.as_ref().unwrap()), "a");
    assert_eq!(line_text(rows[1].left.as_ref().unwrap()), "b");
}

#[test]
fn a_lone_carriage_return_is_not_a_line_break() {
    // Git (and so the saved side) only breaks lines at "\n".
    let r = side_by_side(&text("a\rb\n"), &text("a\rB\n"), false);
    let blocks = blocks(&r);
    let rows = block_rows(&blocks[0]);
    assert_eq!(kinds(rows), [RowKind::Changed]);
    assert_eq!(line_text(rows[0].left.as_ref().unwrap()), "a\rb");
}

#[test]
fn binary_side() {
    assert_eq!(
        side_by_side(&FileContent::Binary, &text("a\n"), false).view,
        DiffView::Binary
    );
    assert_eq!(
        side_by_side(&text("a\n"), &FileContent::Binary, false).view,
        DiffView::Binary
    );
    let r = side_by_side(&FileContent::Missing, &FileContent::Binary, false);
    assert_eq!(r.view, DiffView::Binary);
    assert!(r.before_missing);
}

#[test]
fn too_many_lines() {
    let before = numbered_lines(6_000, None);
    let after = numbered_lines(6_000, Some(3_000));

    let r = side_by_side(&text(&before), &text(&after), false);
    assert_eq!(
        r.view,
        DiffView::TooLarge {
            lines: 6_000,
            bytes: after.len() as u64
        }
    );

    let forced = side_by_side(&text(&before), &text(&after), true);
    assert_eq!(blocks(&forced).len(), 3);
}

#[test]
fn too_many_bytes_in_text() {
    let big = "x".repeat(MAX_DIFF_BYTES as usize + 1);
    let r = side_by_side(&text(&big), &text("y"), false);
    assert_eq!(
        r.view,
        DiffView::TooLarge {
            lines: 1,
            bytes: MAX_DIFF_BYTES + 1
        }
    );
    assert!(matches!(
        side_by_side(&text(&big), &text("y"), true).view,
        DiffView::Blocks(_)
    ));
}

#[test]
fn too_large_content() {
    let r = side_by_side(&FileContent::TooLarge { bytes: 5 }, &text("a\nb\n"), false);
    assert_eq!(r.view, DiffView::TooLarge { lines: 2, bytes: 5 });

    // Forcing cannot conjure up content that was never read.
    let r = side_by_side(
        &text("a\n"),
        &FileContent::TooLarge { bytes: 9_000_000 },
        true,
    );
    assert_eq!(
        r.view,
        DiffView::TooLarge {
            lines: 1,
            bytes: 9_000_000
        }
    );
}

#[test]
fn uneven_replace() {
    // Two old lines replaced by three new ones.
    let r = side_by_side(&text("a\nb\n"), &text("x\ny\nz\n"), false);
    let rows = block_rows(&blocks(&r)[0]);
    assert_eq!(
        kinds(rows),
        [RowKind::Changed, RowKind::Changed, RowKind::Added]
    );
    assert!(rows[2].left.is_none());
    assert_eq!(line_text(rows[2].right.as_ref().unwrap()), "z");

    // The other way round, the leftover is Removed.
    let r = side_by_side(&text("x\ny\nz\n"), &text("a\nb\n"), false);
    let rows = block_rows(&blocks(&r)[0]);
    assert_eq!(
        kinds(rows),
        [RowKind::Changed, RowKind::Changed, RowKind::Removed]
    );
    assert!(rows[2].right.is_none());
    assert_eq!(line_text(rows[2].left.as_ref().unwrap()), "z");
}

#[test]
fn line_numbers_follow_each_side() {
    let r = side_by_side(&text("a\nb\nc\nz\n"), &text("a\nx\ny\nw\nz\n"), false);
    let rows = block_rows(&blocks(&r)[0]);
    let numbers: Vec<(Option<u32>, Option<u32>)> = rows
        .iter()
        .map(|row| {
            (
                row.left.as_ref().map(|l| l.number),
                row.right.as_ref().map(|l| l.number),
            )
        })
        .collect();
    assert_eq!(
        numbers,
        [
            (Some(1), Some(1)),
            (Some(2), Some(2)),
            (Some(3), Some(3)),
            (None, Some(4)),
            (Some(4), Some(5)),
        ]
    );
}

#[test]
fn word_emphasis() {
    let r = side_by_side(&text("let gap = 24;\n"), &text("let gap = 12;\n"), false);
    let rows = block_rows(&blocks(&r)[0]);
    assert_eq!(kinds(rows), [RowKind::Changed]);

    let left = rows[0].left.as_ref().unwrap();
    let right = rows[0].right.as_ref().unwrap();
    assert_eq!(line_text(left), "let gap = 24;");
    assert_eq!(line_text(right), "let gap = 12;");
    assert_eq!(emphasized(left), ["24"]);
    assert_eq!(emphasized(right), ["12"]);
    // Neighbouring segments with the same emphasis are merged.
    assert_eq!(left.segs.len(), 3);
    assert_eq!(right.segs.len(), 3);
}

#[test]
fn plain_rows_are_one_plain_segment() {
    let r = side_by_side(&text("keep\ngone\n"), &text("keep\nnew\n"), false);
    let rows = block_rows(&blocks(&r)[0]);
    let keep = rows[0].left.as_ref().unwrap();
    assert_eq!(keep.segs.len(), 1);
    assert!(!keep.segs[0].emphasized);

    let r = side_by_side(&text("keep\ngone\n"), &text("keep\n"), false);
    let rows = block_rows(&blocks(&r)[0]);
    assert_eq!(rows[1].kind, RowKind::Removed);
    let gone = rows[1].left.as_ref().unwrap();
    assert_eq!(gone.segs.len(), 1);
    assert!(!gone.segs[0].emphasized);
}

#[test]
fn changed_rows_cope_with_empty_lines_and_non_ascii_text() {
    let r = side_by_side(&text("\ncafé açaí\n"), &text("x\ncafé açaí!\n"), false);
    let rows = block_rows(&blocks(&r)[0]);
    assert_eq!(kinds(rows), [RowKind::Changed, RowKind::Changed]);

    // An empty line still has its one (empty) segment.
    let empty = rows[0].left.as_ref().unwrap();
    assert_eq!(empty.segs.len(), 1);
    assert_eq!(line_text(empty), "");
    assert!(!empty.segs[0].emphasized);
    assert_eq!(emphasized(rows[0].right.as_ref().unwrap()), ["x"]);

    assert_eq!(
        emphasized(rows[1].left.as_ref().unwrap()),
        Vec::<&str>::new()
    );
    assert_eq!(emphasized(rows[1].right.as_ref().unwrap()), ["!"]);
    assert_eq!(line_text(rows[1].right.as_ref().unwrap()), "café açaí!");
}
