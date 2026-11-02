//! The comparison view's widget (spec §6.4): a Raven custom widget that paints a comparison as
//! two columns, Before on the left and After on the right, each under its header line.
//!
//! One widget draws both columns from one [`PaneModel`], so they scroll together by
//! construction. Lines are in Raven's fixed-width font, so a character is one column wide and
//! the changed words line up with their highlight.
//!
//! The state sits behind an `Rc` shared by the paint and event callbacks. No borrow of it is
//! held while the "Show anyway" callback runs, so the owner may call back into the pane from it.

use std::cell::RefCell;
use std::rc::Rc;

use savestates_core::{DiffBlock, DiffLine, DiffResult, DiffRow, DiffView, RowKind};
use savestates_ui_raven::{
    Event, MouseButton, Painter, Rgba, Widget, custom, mono_text_size, text_width, tr, tr_plural,
};

use super::model::{PaneModel, PaneRow};
use crate::theme::{self, with_alpha};
use crate::tr_key;

/// The size of the lines' text.
const CODE_SIZE: f32 = 13.0;
/// The height of every row.
const ROW_H: f32 = 20.0;
/// The header line at the top of each column ("ANTES · …").
const HEADER_H: f32 = 30.0;
const HEADER_SIZE: f32 = 11.0;
/// Where the rows start: under the header and its line.
const ROWS_TOP: f32 = HEADER_H + 1.0;
/// Notes: a column whose file does not exist, a placeholder row, the centered messages.
const NOTE_SIZE: f32 = 13.0;
const PLACEHOLDER_SIZE: f32 = 12.0;
/// The small note after a line that lost or gained only its final line break.
const NEWLINE_NOTE_SIZE: f32 = 11.0;
/// Between the two columns.
const COLUMN_GAP: f32 = 10.0;
const CORNER_RADIUS: f32 = 6.0;
/// Inside a column: before the line numbers, between them and the text, after the text.
const PAD: f32 = 10.0;
const GUTTER_GAP: f32 = 12.0;
/// A tab shows as this many spaces.
const TAB_WIDTH: usize = 4;
/// Full-line backgrounds of removed and added lines, and the stronger one of changed words.
const LINE_ALPHA: f32 = 0.15;
const WORD_ALPHA: f32 = 0.35;
/// Diagonal stripes where a side has no line.
const HATCH_STEP: f32 = 7.0;
const HATCH_ALPHA: f32 = 0.55;
/// One turn of the wheel scrolls this many rows, or this many characters sideways.
const WHEEL_ROWS: f32 = 3.0;
const WHEEL_COLUMNS: f32 = 6.0;
/// The scroll bar along the right column.
const SCROLLBAR_W: f32 = 6.0;
const SCROLLBAR_INSET: f32 = 3.0;
const MIN_THUMB_H: f32 = 24.0;
/// The "Show anyway" button.
const BUTTON_PAD_X: f32 = 16.0;
const BUTTON_H: f32 = 30.0;
/// Space kept around a centered message.
const MESSAGE_MARGIN: f32 = 24.0;

/// A comparison to show, with what names its two sides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneInput {
    pub result: DiffResult,
    /// The column headers: "Antes · <save>", "Depois · Agora"...
    pub before_title: String,
    pub after_title: String,
    /// The text key shown in the After column when the file does not exist there
    /// (`compare.was_deleted`, or `compare.no_longer_exists` against Now).
    pub after_missing_key: &'static str,
    /// Whether the Before and the After file end without a line break after their last line.
    pub no_final_newline: (bool, bool),
}

/// The comparison view of one project tab. Cloning gives another handle to the same widget.
#[derive(Clone)]
pub struct DiffPane {
    shared: Rc<Shared>,
}

/// A new comparison view, without a parent yet: put it in an `Overlay` or a `Border` to fill
/// it. It shows nothing until [`DiffPane::show`]. `on_show_anyway` runs when the user asks to
/// see a comparison that was too large.
pub fn new(on_show_anyway: impl FnMut() + 'static) -> (DiffPane, Widget) {
    let widget = custom::create();
    let shared = Rc::new(Shared {
        inner: RefCell::new(Inner {
            content: Content::Note(String::new()),
            scroll_x: 0.0,
            pointer: None,
            drag: None,
            layout: Layout::default(),
        }),
        on_show_anyway: RefCell::new(Box::new(on_show_anyway)),
        widget,
    });
    custom::on_paint(widget, {
        let shared = shared.clone();
        move |p, w, h| shared.paint(p, w, h)
    });
    custom::on_event(widget, {
        let shared = shared.clone();
        move |ev| shared.event(ev)
    });
    (DiffPane { shared }, widget)
}

impl DiffPane {
    /// Shows a comparison. With `keep_view` (the same file compared again, after it changed on
    /// disk), the scroll and the opened blocks stay as they were; when nothing changed at all,
    /// nothing happens.
    pub fn show(&self, input: PaneInput, keep_view: bool) {
        {
            let mut inner = self.shared.inner.borrow_mut();
            let mut model = match &inner.content {
                Content::Diff(shown) if keep_view && shown.input == input => return,
                Content::Diff(shown) if keep_view => {
                    let mut model = PaneModel::from_result(&input.result, ROW_H);
                    model.keep_view_of(&shown.model);
                    model
                }
                _ => {
                    inner.scroll_x = 0.0;
                    PaneModel::from_result(&input.result, ROW_H)
                }
            };
            model.scroll_by(0.0, inner.layout.view_h);
            inner.drag = None;
            inner.content = Content::Diff(Box::new(Shown::new(input, model)));
        }
        self.shared.widget.invalidate();
    }

    /// Shows a note in the middle instead of a comparison ("" for nothing at all).
    pub fn show_note(&self, text: &str) {
        {
            let mut inner = self.shared.inner.borrow_mut();
            inner.content = Content::Note(text.to_owned());
            inner.scroll_x = 0.0;
            inner.drag = None;
        }
        self.shared.widget.invalidate();
    }
}

struct Shared {
    inner: RefCell<Inner>,
    on_show_anyway: RefCell<Box<dyn FnMut()>>,
    widget: Widget,
}

struct Inner {
    content: Content,
    /// How far the lines are scrolled sideways (both columns together).
    scroll_x: f32,
    /// The pointer, while it is over the widget.
    pointer: Option<(f32, f32)>,
    /// While the scroll bar's thumb is dragged: the pointer's y and the scroll when it started.
    drag: Option<(f32, f32)>,
    /// Where the last paint put things, for the input that follows it.
    layout: Layout,
}

enum Content {
    /// A note in the middle, or nothing ("").
    Note(String),
    Diff(Box<Shown>),
}

/// A comparison on show.
struct Shown {
    input: PaneInput,
    model: PaneModel,
    /// The longest line, in characters (tabs counted as spaces): how far it scrolls sideways.
    widest: usize,
    /// The digits of the largest line number: how wide the line numbers are.
    digits: usize,
}

impl Shown {
    fn new(input: PaneInput, model: PaneModel) -> Shown {
        let (mut widest, mut largest) = (0, 0);
        if let DiffView::Blocks(blocks) = &input.result.view {
            for block in blocks {
                let (DiffBlock::Shown(rows) | DiffBlock::Collapsed(rows)) = block;
                for line in rows
                    .iter()
                    .flat_map(|r| [r.left.as_ref(), r.right.as_ref()])
                {
                    let Some(line) = line else { continue };
                    largest = largest.max(line.number);
                    widest = widest.max(line_columns(line));
                }
            }
        }
        Shown {
            input,
            model,
            widest,
            digits: largest.max(1).to_string().len(),
        }
    }
}

/// A rectangle: x, y, width, height.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Rect(f32, f32, f32, f32);

impl Rect {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.0 && x < self.0 + self.2 && y >= self.1 && y < self.1 + self.3
    }
}

/// What the last paint measured and placed.
#[derive(Clone, Copy, Debug, Default)]
struct Layout {
    /// The height of the rows' area under the headers.
    view_h: f32,
    char_w: f32,
    /// How far the lines can scroll sideways.
    max_scroll_x: f32,
    button: Option<Rect>,
    track: Option<Rect>,
    thumb: Option<Rect>,
}

impl Inner {
    /// Whether the comparison was too large to show, so the user may ask for it anyway.
    fn offers_show_anyway(&self) -> bool {
        matches!(&self.content, Content::Diff(shown)
            if matches!(shown.input.result.view, DiffView::TooLarge { .. }))
    }

    /// What the pointer and the scroll make look different, to repaint only when it changes.
    fn looks(&self) -> Looks {
        let layout = &self.layout;
        let (scroll_y, rows, hovered_row) = match &self.content {
            Content::Diff(shown) => {
                let model = &shown.model;
                let hovered = self
                    .pointer
                    .and_then(|(_, y)| model.row_at(y - ROWS_TOP))
                    .filter(|&i| matches!(model.rows[i], PaneRow::Placeholder { .. }));
                (model.scroll_y, model.rows.len(), hovered)
            }
            Content::Note(_) => (0.0, 0, None),
        };
        let over = |rect: Option<Rect>| {
            self.pointer
                .is_some_and(|(x, y)| rect.is_some_and(|r| r.contains(x, y)))
        };
        Looks {
            scroll: (scroll_y, self.scroll_x),
            rows,
            hovered_row,
            over_button: over(layout.button),
            over_thumb: over(layout.thumb) || self.drag.is_some(),
        }
    }
}

/// What the pointer and the scroll change on screen.
#[derive(Clone, Copy, PartialEq)]
struct Looks {
    scroll: (f32, f32),
    rows: usize,
    hovered_row: Option<usize>,
    over_button: bool,
    over_thumb: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Column {
    Before,
    After,
}

impl Shared {
    fn paint(&self, p: &mut Painter, w: f32, h: f32) {
        let Ok(mut guard) = self.inner.try_borrow_mut() else {
            return;
        };
        let inner = &mut *guard;
        inner.layout = Layout {
            char_w: mono_text_size(CODE_SIZE, "0").0.max(1.0),
            ..Layout::default()
        };
        p.push_clip(0.0, 0.0, w, h);
        p.rect(0.0, 0.0, w, h, 0.0, theme::BG);
        let message = match &inner.content {
            Content::Note(text) => Some(text.clone()),
            Content::Diff(shown) => match &shown.input.result.view {
                DiffView::Blocks(_) => None,
                DiffView::Identical => Some(tr("compare.identical")),
                DiffView::OnlyLineEndings => Some(tr("compare.only_line_endings")),
                DiffView::Binary => Some(tr("compare.binary")),
                DiffView::TooLarge { .. } => Some(tr("compare.too_large")),
            },
        };
        match message {
            None => paint_columns(p, inner, w, h),
            Some(text) => {
                let bottom = paint_message(p, w, h, &text);
                if inner.offers_show_anyway() {
                    let label = tr("compare.show_anyway");
                    let button = paint_button(p, w, bottom + 12.0, &label, inner.pointer);
                    inner.layout.button = Some(button);
                }
            }
        }
        p.pop_clip();
    }

    fn event(&self, ev: &Event) -> bool {
        let mut show_anyway = false;
        let (consumed, repaint) = {
            let Ok(mut guard) = self.inner.try_borrow_mut() else {
                return false;
            };
            let inner = &mut *guard;
            let layout = inner.layout;
            let before = inner.looks();
            let consumed = match *ev {
                Event::Wheel { wx, wy, .. } => {
                    if let Content::Diff(shown) = &mut inner.content {
                        let dy = -wy * WHEEL_ROWS * shown.model.row_h;
                        shown.model.scroll_by(dy, layout.view_h);
                    }
                    inner.scroll_x = (inner.scroll_x - wx * WHEEL_COLUMNS * layout.char_w)
                        .clamp(0.0, layout.max_scroll_x);
                    true
                }
                Event::MouseDown {
                    x,
                    y,
                    button: MouseButton::Left,
                    ..
                } => {
                    inner.pointer = Some((x, y));
                    if layout.button.is_some_and(|b| b.contains(x, y)) {
                        show_anyway = true;
                    } else if let Content::Diff(shown) = &mut inner.content {
                        press(shown, &mut inner.drag, &layout, x, y);
                    }
                    true
                }
                Event::MouseUp {
                    button: MouseButton::Left,
                    ..
                } => {
                    inner.drag = None;
                    true
                }
                Event::MouseMove { x, y, .. } => {
                    inner.pointer = Some((x, y));
                    if let (Some((start_y, start_scroll)), Content::Diff(shown)) =
                        (inner.drag, &mut inner.content)
                    {
                        drag_thumb(&mut shown.model, &layout, start_y, start_scroll, y);
                    }
                    true
                }
                Event::Leave => {
                    inner.pointer = None;
                    inner.drag = None;
                    false
                }
                _ => false,
            };
            (consumed, inner.looks() != before)
        };
        if repaint {
            self.widget.invalidate();
        }
        if show_anyway && let Ok(mut on_show_anyway) = self.on_show_anyway.try_borrow_mut() {
            on_show_anyway();
        }
        consumed
    }
}

/// A press in the comparison: on the scroll bar's thumb it starts a drag, elsewhere on its
/// track it scrolls a page, and on a placeholder row it opens the lines folded there.
fn press(shown: &mut Shown, drag: &mut Option<(f32, f32)>, layout: &Layout, x: f32, y: f32) {
    if layout.thumb.is_some_and(|t| t.contains(x, y)) {
        *drag = Some((y, shown.model.scroll_y));
        return;
    }
    if let (Some(track), Some(thumb)) = (layout.track, layout.thumb)
        && track.contains(x, y)
    {
        let page = if y < thumb.1 {
            -layout.view_h
        } else {
            layout.view_h
        };
        shown.model.scroll_by(page, layout.view_h);
        return;
    }
    let row = shown
        .model
        .row_at(y - ROWS_TOP)
        .map(|i| &shown.model.rows[i]);
    if let Some(&PaneRow::Placeholder { block, .. }) = row {
        shown.model.expand(block);
    }
}

/// Moves the scroll with the thumb: the pointer went from `start_y` to `y`.
fn drag_thumb(model: &mut PaneModel, layout: &Layout, start_y: f32, start_scroll: f32, y: f32) {
    let (Some(track), Some(thumb)) = (layout.track, layout.thumb) else {
        return;
    };
    let content_h = model.rows.len() as f32 * model.row_h;
    let travel = track.3 - thumb.3;
    if travel <= 0.0 {
        return;
    }
    let target = start_scroll + (y - start_y) * (content_h - layout.view_h) / travel;
    model.scroll_by(target - model.scroll_y, layout.view_h);
}

/// The two columns of a comparison made of rows.
fn paint_columns(p: &mut Painter, inner: &mut Inner, w: f32, h: f32) {
    let Content::Diff(shown) = &mut inner.content else {
        return;
    };
    let char_w = inner.layout.char_w;
    let line_h = mono_text_size(CODE_SIZE, "Ag").1;
    let view_h = (h - ROWS_TOP).max(0.0);
    shown.model.scroll_by(0.0, view_h);
    let columns = columns(w);
    let gutter_w = PAD + shown.digits as f32 * char_w + GUTTER_GAP;
    let text_w = (columns[0].1 - gutter_w - PAD).max(0.0);
    let max_scroll_x = (shown.widest as f32 * char_w - text_w).max(0.0);
    inner.scroll_x = inner.scroll_x.clamp(0.0, max_scroll_x);
    inner.layout.view_h = view_h;
    inner.layout.max_scroll_x = max_scroll_x;

    let hovered_row = inner
        .pointer
        .and_then(|(_, y)| shown.model.row_at(y - ROWS_TOP));
    let geometry = Geometry {
        char_w,
        line_h,
        gutter_w,
        text_w,
        scroll_x: inner.scroll_x,
        view_h,
    };
    for (column, (x, col_w)) in [Column::Before, Column::After].into_iter().zip(columns) {
        paint_column(p, shown, &geometry, column, x, col_w, h, hovered_row);
    }
    let (track, thumb) = scrollbar(&shown.model, columns[1], view_h);
    if let Some(thumb) = thumb {
        let hot = inner.drag.is_some() || inner.pointer.is_some_and(|(x, y)| thumb.contains(x, y));
        let color = if hot {
            theme::MUTED_TEXT
        } else {
            theme::BORDER
        };
        p.rect(thumb.0, thumb.1, thumb.2, thumb.3, SCROLLBAR_W / 2.0, color);
    }
    inner.layout.track = track;
    inner.layout.thumb = thumb;
}

/// Measures shared by both columns.
struct Geometry {
    char_w: f32,
    line_h: f32,
    /// From a column's left edge to where its text starts.
    gutter_w: f32,
    /// The width the text has in a column.
    text_w: f32,
    scroll_x: f32,
    view_h: f32,
}

#[allow(clippy::too_many_arguments)]
fn paint_column(
    p: &mut Painter,
    shown: &Shown,
    g: &Geometry,
    column: Column,
    x: f32,
    col_w: f32,
    h: f32,
    hovered_row: Option<usize>,
) {
    let input = &shown.input;
    p.rect(x, 0.0, col_w, h, CORNER_RADIUS, theme::CODE_BG);
    p.rect_outline(x, 0.0, col_w, h, CORNER_RADIUS, 1.0, theme::BORDER);
    let title = match column {
        Column::Before => &input.before_title,
        Column::After => &input.after_title,
    };
    let title = fit_text(&title.to_uppercase(), col_w - 2.0 * PAD, |s| {
        text_width(HEADER_SIZE, s)
    });
    p.text(
        HEADER_SIZE,
        x + PAD,
        (HEADER_H - HEADER_SIZE * 1.3) / 2.0,
        &title,
        theme::MUTED_TEXT,
    );
    p.line(x, HEADER_H, x + col_w, HEADER_H, 1.0, theme::BORDER);

    p.push_clip(x + 1.0, ROWS_TOP, col_w - 2.0, g.view_h);
    let missing = match column {
        Column::Before => input
            .result
            .before_missing
            .then_some(tr_key("compare.did_not_exist")),
        Column::After => input
            .result
            .after_missing
            .then_some(input.after_missing_key),
    };
    if let Some(key) = missing {
        let text = tr(key);
        let lines = wrap_text(&text, col_w - 2.0 * MESSAGE_MARGIN, |s| {
            text_width(NOTE_SIZE, s)
        });
        paint_lines(
            p,
            x + col_w / 2.0,
            ROWS_TOP + g.view_h / 2.0,
            &lines,
            theme::MUTED_TEXT,
        );
        p.pop_clip();
        return;
    }
    let model = &shown.model;
    for i in model.visible(g.view_h) {
        let y = ROWS_TOP + i as f32 * model.row_h - model.scroll_y;
        match &model.rows[i] {
            PaneRow::Row(row) => paint_line(p, shown, g, column, row, x, col_w, y),
            PaneRow::Placeholder { hidden, .. } => {
                let hovered = hovered_row == Some(i);
                paint_placeholder(p, x, col_w, y, model.row_h, *hidden, hovered);
            }
        }
    }
    p.pop_clip();
}

#[allow(clippy::too_many_arguments)]
fn paint_line(
    p: &mut Painter,
    shown: &Shown,
    g: &Geometry,
    column: Column,
    row: &DiffRow,
    x: f32,
    col_w: f32,
    y: f32,
) {
    let row_h = shown.model.row_h;
    let (line, tint) = match column {
        Column::Before => (
            row.left.as_ref(),
            matches!(row.kind, RowKind::Removed | RowKind::Changed).then_some(theme::REMOVED),
        ),
        Column::After => (
            row.right.as_ref(),
            matches!(row.kind, RowKind::Added | RowKind::Changed).then_some(theme::ADDED),
        ),
    };
    let Some(line) = line else {
        hatch(p, x + 1.0, y, col_w - 2.0, row_h);
        return;
    };
    if let Some(tint) = tint {
        p.rect(
            x + 1.0,
            y,
            col_w - 2.0,
            row_h,
            0.0,
            with_alpha(tint, LINE_ALPHA),
        );
    }
    let text_y = y + (row_h - g.line_h) / 2.0;
    let number = line.number.to_string();
    let number_x = x + PAD + shown.digits.saturating_sub(number.len()) as f32 * g.char_w;
    p.mono_text(CODE_SIZE, number_x, text_y, &number, theme::MUTED_TEXT);

    let text_x = x + g.gutter_w;
    p.push_clip(text_x, y, g.text_w, row_h);
    let mut column_at = 0;
    for seg in &line.segs {
        let text = expand_tabs(&seg.text);
        let len = text.chars().count();
        let seg_x = text_x + column_at as f32 * g.char_w - g.scroll_x;
        if seg.emphasized
            && let Some(tint) = tint
        {
            let seg_w = len as f32 * g.char_w;
            p.rect(
                seg_x,
                y + 2.0,
                seg_w,
                row_h - 4.0,
                2.0,
                with_alpha(tint, WORD_ALPHA),
            );
        }
        let (skip, take) = visible_chars(len, seg_x, text_x, g.text_w, g.char_w);
        if take > 0 {
            let part: String = text.chars().skip(skip).take(take).collect();
            let part_x = seg_x + skip as f32 * g.char_w;
            p.mono_text(CODE_SIZE, part_x, text_y, &part, theme::TEXT);
        }
        column_at += len;
    }
    let lacks_newline = match column {
        Column::Before => shown.input.no_final_newline.0,
        Column::After => shown.input.no_final_newline.1,
    };
    if lacks_newline && only_the_line_break_differs(row) {
        let note = tr("compare.no_newline_at_end");
        let note_x = text_x + (column_at + 2) as f32 * g.char_w - g.scroll_x;
        let note_y = y + (row_h - NEWLINE_NOTE_SIZE * 1.3) / 2.0;
        p.text(NEWLINE_NOTE_SIZE, note_x, note_y, &note, theme::MUTED_TEXT);
    }
    p.pop_clip();
}

/// A row standing for unchanged lines folded away: "… 46 linhas sem alteração …".
fn paint_placeholder(
    p: &mut Painter,
    x: f32,
    col_w: f32,
    y: f32,
    row_h: f32,
    hidden: usize,
    hovered: bool,
) {
    let (background, color) = if hovered {
        (theme::HOVERED, theme::LINE)
    } else {
        (theme::SURFACE, theme::MUTED_TEXT)
    };
    p.rect(x + 1.0, y, col_w - 2.0, row_h, 0.0, background);
    let count = i64::try_from(hidden).unwrap_or(i64::MAX);
    let text = tr_plural("compare.hidden_lines", count);
    let text_w = text_width(PLACEHOLDER_SIZE, &text);
    let text_y = y + (row_h - PLACEHOLDER_SIZE * 1.3) / 2.0;
    p.text(
        PLACEHOLDER_SIZE,
        x + (col_w - text_w) / 2.0,
        text_y,
        &text,
        color,
    );
}

/// Diagonal stripes over a rectangle: where a side has no line facing the other side's.
fn hatch(p: &mut Painter, x: f32, y: f32, w: f32, h: f32) {
    p.push_clip(x, y, w, h);
    let color = with_alpha(theme::BORDER, HATCH_ALPHA);
    let mut offset = -h;
    while offset < w {
        p.line(x + offset, y + h, x + offset + h, y, 1.0, color);
        offset += HATCH_STEP;
    }
    p.pop_clip();
}

/// A message centered in the widget, wrapped to its width. Returns the y below its last line.
fn paint_message(p: &mut Painter, w: f32, h: f32, text: &str) -> f32 {
    if text.is_empty() {
        return h / 2.0;
    }
    let lines = wrap_text(text, w - 2.0 * MESSAGE_MARGIN, |s| text_width(NOTE_SIZE, s));
    paint_lines(p, w / 2.0, h / 2.0 - 20.0, &lines, theme::MUTED_TEXT)
}

/// `lines` centered on (`cx`, `cy`). Returns the y below the last one.
fn paint_lines(p: &mut Painter, cx: f32, cy: f32, lines: &[String], color: Rgba) -> f32 {
    let line_h = NOTE_SIZE * 1.5;
    let top = cy - lines.len() as f32 * line_h / 2.0;
    for (i, line) in lines.iter().enumerate() {
        let x = cx - text_width(NOTE_SIZE, line) / 2.0;
        p.text(NOTE_SIZE, x, top + i as f32 * line_h, line, color);
    }
    top + lines.len() as f32 * line_h
}

/// A button centered across the widget at `y`. Returns where it is, for clicks.
fn paint_button(p: &mut Painter, w: f32, y: f32, label: &str, pointer: Option<(f32, f32)>) -> Rect {
    let label_w = text_width(NOTE_SIZE, label);
    let bw = label_w + 2.0 * BUTTON_PAD_X;
    let rect = Rect((w - bw) / 2.0, y, bw, BUTTON_H);
    let hovered = pointer.is_some_and(|(x, y)| rect.contains(x, y));
    let background = if hovered {
        theme::HOVERED
    } else {
        theme::SURFACE
    };
    p.rect(rect.0, rect.1, rect.2, rect.3, CORNER_RADIUS, background);
    p.rect_outline(
        rect.0,
        rect.1,
        rect.2,
        rect.3,
        CORNER_RADIUS,
        1.0,
        theme::BORDER,
    );
    let text_y = y + (BUTTON_H - NOTE_SIZE * 1.3) / 2.0;
    p.text(NOTE_SIZE, rect.0 + BUTTON_PAD_X, text_y, label, theme::TEXT);
    rect
}

/// The x and the width of the Before and the After column in a widget `w` wide.
fn columns(w: f32) -> [(f32, f32); 2] {
    let col_w = ((w - COLUMN_GAP) / 2.0).max(0.0);
    [(0.0, col_w), (col_w + COLUMN_GAP, col_w)]
}

/// The scroll bar's track and thumb along the right edge of the After column (`(x, width)`),
/// when the rows do not all fit in `view_h`.
fn scrollbar(model: &PaneModel, after: (f32, f32), view_h: f32) -> (Option<Rect>, Option<Rect>) {
    let content_h = model.rows.len() as f32 * model.row_h;
    if content_h <= view_h || view_h <= 2.0 * SCROLLBAR_INSET {
        return (None, None);
    }
    let x = after.0 + after.1 - SCROLLBAR_W - SCROLLBAR_INSET;
    let track = Rect(
        x,
        ROWS_TOP + SCROLLBAR_INSET,
        SCROLLBAR_W,
        view_h - 2.0 * SCROLLBAR_INSET,
    );
    let thumb_h = (track.3 * view_h / content_h).clamp(MIN_THUMB_H.min(track.3), track.3);
    let travel = track.3 - thumb_h;
    let thumb_y = track.1 + travel * model.scroll_y / (content_h - view_h);
    (Some(track), Some(Rect(x, thumb_y, SCROLLBAR_W, thumb_h)))
}

/// Of a piece of text `len` characters long drawn from `seg_x`, the characters that fall in the
/// area from `area_x`, `area_w` wide, as (how many to skip, how many to draw).
fn visible_chars(len: usize, seg_x: f32, area_x: f32, area_w: f32, char_w: f32) -> (usize, usize) {
    if char_w <= 0.0 {
        return (0, 0);
    }
    let skip = ((area_x - seg_x) / char_w).floor().max(0.0) as usize;
    if skip >= len {
        return (len, 0);
    }
    let first_x = seg_x + skip as f32 * char_w;
    let room = (area_x + area_w - first_x).max(0.0);
    let take = ((room / char_w).ceil() as usize).min(len - skip);
    (skip, take)
}

/// How many columns a line takes, tabs counted as [`TAB_WIDTH`] spaces.
fn line_columns(line: &DiffLine) -> usize {
    line.segs
        .iter()
        .map(|seg| {
            seg.text
                .chars()
                .map(|c| if c == '\t' { TAB_WIDTH } else { 1 })
                .sum::<usize>()
        })
        .sum()
}

fn expand_tabs(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains('\t') {
        std::borrow::Cow::Owned(text.replace('\t', &" ".repeat(TAB_WIDTH)))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// Whether both sides of a changed row read the same: only the line break after it differs
/// (the last line of a file that gained or lost its final line break).
fn only_the_line_break_differs(row: &DiffRow) -> bool {
    let text = |line: &DiffLine| {
        line.segs
            .iter()
            .map(|s| s.text.as_str())
            .collect::<String>()
    };
    match (row.kind, &row.left, &row.right) {
        (RowKind::Changed, Some(left), Some(right)) => text(left) == text(right),
        _ => false,
    }
}

/// `text` as it fits in `max_w` by `measure`: whole, or cut and ended with `…`.
fn fit_text(text: &str, max_w: f32, measure: impl Fn(&str) -> f32) -> String {
    if measure(text) <= max_w {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    for keep in (0..chars.len()).rev() {
        let cut: String = chars[..keep]
            .iter()
            .collect::<String>()
            .trim_end()
            .to_owned()
            + "…";
        if measure(&cut) <= max_w {
            return cut;
        }
    }
    String::new()
}

/// `text` broken at spaces into lines no wider than `max_w` by `measure`. A word wider than a
/// line is cut to fit.
fn wrap_text(text: &str, max_w: f32, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let longer = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if measure(&longer) <= max_w || line.is_empty() {
            line = longer;
        } else {
            lines.push(std::mem::take(&mut line));
            line = word.to_owned();
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
        .into_iter()
        .map(|line| fit_text(&line, max_w, &measure))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use savestates_core::{FileContent, Seg, side_by_side};

    /// One unit of width per character.
    fn chars(s: &str) -> f32 {
        s.chars().count() as f32
    }

    #[test]
    fn the_columns_share_the_width_around_a_gap() {
        let [before, after] = columns(410.0);
        assert_eq!(before, (0.0, 200.0));
        assert_eq!(after, (210.0, 200.0));
        assert_eq!(columns(4.0), [(0.0, 0.0), (COLUMN_GAP, 0.0)]);
    }

    #[test]
    fn only_the_characters_in_view_are_drawn() {
        // Ten characters of width 2 from x = 0, seen through 0..6: the first three.
        assert_eq!(visible_chars(10, 0.0, 0.0, 6.0, 2.0), (0, 3));
        // Scrolled sideways by 5: the third character is half in view, so it is drawn.
        assert_eq!(visible_chars(10, -5.0, 0.0, 6.0, 2.0), (2, 4));
        // Entirely to the left, or to the right.
        assert_eq!(visible_chars(10, -40.0, 0.0, 6.0, 2.0), (10, 0));
        assert_eq!(visible_chars(10, 7.0, 0.0, 6.0, 2.0), (0, 0));
        // A short piece is drawn whole.
        assert_eq!(visible_chars(2, 1.0, 0.0, 60.0, 2.0), (0, 2));
    }

    #[test]
    fn tabs_count_as_four_columns() {
        let line = DiffLine {
            number: 1,
            segs: vec![
                Seg {
                    text: "\tgap:".to_owned(),
                    emphasized: false,
                },
                Seg {
                    text: " 1ção".to_owned(),
                    emphasized: true,
                },
            ],
        };
        assert_eq!(line_columns(&line), 4 + 4 + 5);
        assert_eq!(expand_tabs("\ta\t"), "    a    ");
    }

    #[test]
    fn a_line_that_only_gained_its_final_line_break_is_told_apart() {
        let rows = |before: &str, after: &str| {
            let r = side_by_side(
                &FileContent::Text(before.to_owned()),
                &FileContent::Text(after.to_owned()),
                false,
            );
            PaneModel::from_result(&r, ROW_H).rows
        };
        let changed = rows("a\nb", "a\nb\n");
        let PaneRow::Row(last) = changed.last().unwrap() else {
            panic!("a row")
        };
        assert!(only_the_line_break_differs(last));
        let changed = rows("a\nb\n", "a\nc\n");
        let PaneRow::Row(last) = changed.last().unwrap() else {
            panic!("a row")
        };
        assert!(!only_the_line_break_differs(last));
    }

    #[test]
    fn titles_and_messages_fit_their_width() {
        assert_eq!(
            fit_text("ANTES · MODO ESCURO", 50.0, chars),
            "ANTES · MODO ESCURO"
        );
        assert_eq!(fit_text("ANTES · MODO ESCURO", 10.0, chars), "ANTES · M…");
        assert_eq!(fit_text("ANTES", 0.5, chars), "");
        assert_eq!(
            wrap_text("Este arquivo foi apagado", 12.0, chars),
            ["Este arquivo", "foi apagado"]
        );
        assert_eq!(wrap_text("Supercalifragilístico", 6.0, chars), ["Super…"]);
        assert!(wrap_text("", 10.0, chars).is_empty());
    }

    #[test]
    fn the_scroll_bar_shows_where_the_view_is() {
        let r = side_by_side(
            &FileContent::Text("a\n".repeat(100)),
            &FileContent::Text(format!("{}b\n", "a\n".repeat(100))),
            true,
        );
        let mut model = PaneModel::from_result(&r, ROW_H);
        model.expand(0);
        let rows = model.rows.len() as f32;
        // Everything fits: no scroll bar.
        assert_eq!(
            scrollbar(&model, (200.0, 100.0), rows * ROW_H),
            (None, None)
        );
        let view_h = 106.0;
        let (track, thumb) = scrollbar(&model, (200.0, 100.0), view_h);
        let (track, thumb) = (track.unwrap(), thumb.unwrap());
        assert_eq!(track.0, 300.0 - SCROLLBAR_W - SCROLLBAR_INSET);
        assert_eq!(thumb.1, track.1);
        assert!(thumb.3 >= MIN_THUMB_H);
        // At the bottom, the thumb reaches the end of the track.
        model.scroll_by(f32::MAX, view_h);
        let (_, thumb) = scrollbar(&model, (200.0, 100.0), view_h);
        let thumb = thumb.unwrap();
        assert!((thumb.1 + thumb.3 - (track.1 + track.3)).abs() < 0.01);
    }
}
