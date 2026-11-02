//! What the comparison view shows, row by row, and where it is scrolled to.

use std::collections::HashSet;
use std::ops::Range;

use savestates_core::{DiffBlock, DiffResult, DiffRow, DiffView};

/// One row of the comparison view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneRow {
    /// A line on each side, or on one of them.
    Row(DiffRow),
    /// Unchanged lines folded away: `hidden` rows of the comparison's block number `block`.
    /// Clicking it shows them.
    Placeholder { block: usize, hidden: usize },
}

/// The rows of a comparison, which folded blocks the user opened, and how far down it is
/// scrolled. Heights are in the view's units; `row_h` is the height of every row.
#[derive(Clone, Debug)]
pub struct PaneModel {
    pub rows: Vec<PaneRow>,
    pub scroll_y: f32,
    pub row_h: f32,
    /// The folded blocks the user opened.
    expanded: HashSet<usize>,
    /// The comparison's blocks, which `rows` is made from.
    blocks: Vec<DiffBlock>,
}

impl PaneModel {
    /// The rows of `r`, with every folded block shown as one placeholder, at the top. A view
    /// that is not made of rows (identical files, a binary file...) has none.
    pub fn from_result(r: &DiffResult, row_h: f32) -> PaneModel {
        let blocks = match &r.view {
            DiffView::Blocks(blocks) => blocks.clone(),
            _ => Vec::new(),
        };
        let mut model = PaneModel {
            rows: Vec::new(),
            scroll_y: 0.0,
            row_h,
            expanded: HashSet::new(),
            blocks,
        };
        model.lay_out();
        model
    }

    /// Shows the rows folded away in block number `block`, in place of its placeholder. Rows
    /// below it move down; the view stays where it was.
    pub fn expand(&mut self, block: usize) {
        let folded = matches!(self.blocks.get(block), Some(DiffBlock::Collapsed(_)));
        if folded && self.expanded.insert(block) {
            self.lay_out();
        }
    }

    /// Scrolls down by `dy` (up when negative), between the top and the point where the last
    /// row reaches the bottom of a view `viewport_h` high.
    pub fn scroll_by(&mut self, dy: f32, viewport_h: f32) {
        let max = (self.rows.len() as f32 * self.row_h - viewport_h).max(0.0);
        self.scroll_y = (self.scroll_y + dy).clamp(0.0, max);
    }

    /// The row at height `y` from the top of the view, if any.
    pub fn row_at(&self, y: f32) -> Option<usize> {
        if y < 0.0 || self.row_h <= 0.0 {
            return None;
        }
        let row = ((y + self.scroll_y) / self.row_h).floor() as usize;
        (row < self.rows.len()).then_some(row)
    }

    /// The rows that show, partly or wholly, in a view `viewport_h` high.
    pub fn visible(&self, viewport_h: f32) -> Range<usize> {
        if self.row_h <= 0.0 {
            return 0..0;
        }
        let count = self.rows.len();
        let start = ((self.scroll_y / self.row_h).floor() as usize).min(count);
        let end = (((self.scroll_y + viewport_h.max(0.0)) / self.row_h).ceil() as usize)
            .clamp(start, count);
        start..end
    }

    /// Keeps how `old` (an earlier comparison of the same file) was being read: the blocks
    /// opened there that are still folded here, and how far it was scrolled (the next
    /// [`scroll_by`](Self::scroll_by) brings that back within the rows).
    pub fn keep_view_of(&mut self, old: &PaneModel) {
        self.expanded = old
            .expanded
            .iter()
            .copied()
            .filter(|&block| matches!(self.blocks.get(block), Some(DiffBlock::Collapsed(_))))
            .collect();
        self.scroll_y = old.scroll_y;
        self.lay_out();
    }

    /// Makes `rows` from the blocks: shown rows as they are, folded ones as one placeholder
    /// unless the user opened them.
    fn lay_out(&mut self) {
        let mut rows = Vec::new();
        for (index, block) in self.blocks.iter().enumerate() {
            match block {
                DiffBlock::Collapsed(hidden) if !self.expanded.contains(&index) => {
                    rows.push(PaneRow::Placeholder {
                        block: index,
                        hidden: hidden.len(),
                    });
                }
                DiffBlock::Shown(shown) | DiffBlock::Collapsed(shown) => {
                    rows.extend(shown.iter().cloned().map(PaneRow::Row));
                }
            }
        }
        self.rows = rows;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use savestates_core::{DiffResult, FileContent, side_by_side};

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

    /// Task 5's 100-line case: line 50 changed, the rest folded away around it.
    fn hundred_lines() -> DiffResult {
        side_by_side(
            &FileContent::Text(numbered_lines(100, None)),
            &FileContent::Text(numbered_lines(100, Some(50))),
            false,
        )
    }

    fn is_row(row: &PaneRow) -> bool {
        matches!(row, PaneRow::Row(_))
    }

    fn left_number(row: &PaneRow) -> Option<u32> {
        match row {
            PaneRow::Row(row) => row.left.as_ref().map(|line| line.number),
            PaneRow::Placeholder { .. } => None,
        }
    }

    #[test]
    fn collapsed_blocks_are_placeholders_that_expand_in_place() {
        let mut model = PaneModel::from_result(&hundred_lines(), 20.0);
        assert_eq!(model.rows.len(), 9);
        assert_eq!(
            model.rows[0],
            PaneRow::Placeholder {
                block: 0,
                hidden: 46
            }
        );
        assert!(model.rows[1..8].iter().all(is_row));
        assert_eq!(left_number(&model.rows[1]), Some(47));
        assert_eq!(
            model.rows[8],
            PaneRow::Placeholder {
                block: 2,
                hidden: 47
            }
        );

        model.expand(0);
        assert_eq!(model.rows.len(), 46 + 7 + 1);
        assert!(model.rows[..53].iter().all(is_row));
        assert_eq!(left_number(&model.rows[0]), Some(1));
        assert_eq!(left_number(&model.rows[45]), Some(46));
        assert_eq!(left_number(&model.rows[46]), Some(47));
        assert_eq!(
            model.rows[53],
            PaneRow::Placeholder {
                block: 2,
                hidden: 47
            }
        );

        // Expanding it again, a block that is shown, or one that does not exist changes nothing.
        model.expand(0);
        model.expand(1);
        model.expand(7);
        assert_eq!(model.rows.len(), 54);
    }

    #[test]
    fn scrolling_stops_at_both_ends() {
        // 9 rows of 20: 180 high.
        let mut model = PaneModel::from_result(&hundred_lines(), 20.0);
        model.scroll_by(-50.0, 100.0);
        assert_eq!(model.scroll_y, 0.0);
        model.scroll_by(30.0, 100.0);
        assert_eq!(model.scroll_y, 30.0);
        model.scroll_by(1_000.0, 100.0);
        assert_eq!(model.scroll_y, 80.0);
        model.scroll_by(-1_000.0, 100.0);
        assert_eq!(model.scroll_y, 0.0);
        // Rows that all fit do not scroll.
        model.scroll_by(10.0, 500.0);
        assert_eq!(model.scroll_y, 0.0);
    }

    #[test]
    fn rows_are_found_by_height_and_only_the_visible_ones_are_listed() {
        let mut model = PaneModel::from_result(&hundred_lines(), 20.0);
        assert_eq!(model.row_at(model.row_h * 2.5), Some(2));
        assert_eq!(model.row_at(0.0), Some(0));
        assert_eq!(model.row_at(-0.5), None);
        assert_eq!(model.row_at(model.row_h * 9.0), None);
        assert_eq!(model.visible(50.0), 0..3);

        // Scrolled, heights are taken from the top of the view.
        model.scroll_by(30.0, 100.0);
        assert_eq!(model.row_at(0.0), Some(1));
        assert_eq!(model.row_at(model.row_h * 2.5), Some(4));
        assert_eq!(model.visible(50.0), 1..4);
        // Never past the last row.
        assert_eq!(model.visible(10_000.0), 1..9);
    }

    #[test]
    fn the_same_file_compared_again_keeps_how_it_was_read() {
        let mut old = PaneModel::from_result(&hundred_lines(), 20.0);
        old.expand(2);
        old.scroll_by(40.0, 100.0);

        // The file changed on disk: line 70 changed too, which makes five blocks.
        let mut edited = numbered_lines(100, Some(50));
        edited = edited.replace("l70\n", "L70\n");
        let newer = side_by_side(
            &FileContent::Text(numbered_lines(100, None)),
            &FileContent::Text(edited),
            false,
        );
        let mut model = PaneModel::from_result(&newer, 20.0);
        model.keep_view_of(&old);
        assert_eq!(model.scroll_y, 40.0);
        // Block 0 is still folded, and the last folded block (4) was never opened.
        assert_eq!(
            model.rows[0],
            PaneRow::Placeholder {
                block: 0,
                hidden: 46
            }
        );
        assert!(matches!(
            model.rows.last(),
            Some(PaneRow::Placeholder { block: 4, .. })
        ));

        // The same comparison again: the opened block stays open.
        let mut again = PaneModel::from_result(&hundred_lines(), 20.0);
        again.keep_view_of(&old);
        assert_eq!(again.rows.len(), old.rows.len());
        assert!(again.rows[8..].iter().all(is_row));
    }

    #[test]
    fn views_without_rows_have_no_rows() {
        let same = side_by_side(
            &FileContent::Text("a\n".to_owned()),
            &FileContent::Text("a\n".to_owned()),
            false,
        );
        let model = PaneModel::from_result(&same, 20.0);
        assert!(model.rows.is_empty());
        assert_eq!(model.visible(100.0), 0..0);
        assert_eq!(model.row_at(5.0), None);
    }
}
