//! The rows of a change list (spec addendum §5): a Raven custom widget that paints each change
//! as a colored symbol (`+` new, `−` deleted, `M` changed, `R` renamed), the file's name and,
//! aligned right in muted text, its folder, shortened in the middle when it is long.
//!
//! A click selects a row, Up and Down move the selection while the list has the keyboard, the
//! wheel and the scroll bar scroll, and a right click selects the row under the pointer, then
//! opens the context menu (when the list has one). Resting the pointer on a symbol shows what it
//! means; on a shortened folder or a cut name, the whole of it. Only the rows in view are
//! painted, so thousands of them cost no more than a screenful.
//!
//! The row logic is in [`Inner`], which measures text through a function so that it is tested
//! without Raven. The state sits behind an `Rc`; Raven's callbacks and the tooltip's timer hold
//! it weakly. No borrow of it is held while `on_select` runs, so the owner may call back into
//! the list from there.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use savestates_core::{Change, ChangeKind};
use savestates_ui_raven::{
    Event, Key, MouseButton, Painter, Rgba, Widget, Window, custom, mono_text_size, task,
    text_width, tr, tr_fmt,
};

use crate::change_rows::{
    RowsModel, Symbol, Zone, fit_end, shorten_middle, split_path, symbol, symbol_text, tooltip_key,
    wrap_text,
};
use crate::theme::{self, with_alpha};

/// The height of every row.
const ROW_H: f32 = 24.0;
/// Text sizes: the name in the theme's size, the folder and the tooltips a little smaller.
const NAME_SIZE: f32 = 14.0;
const FOLDER_SIZE: f32 = 12.0;
const SYMBOL_SIZE: f32 = 13.0;
const TIP_SIZE: f32 = 12.0;
/// A line of text is this many times its size high.
const LINE_HEIGHT: f32 = 1.3;
/// The list's outline, which the rows stay inside.
const INSET: f32 = 1.0;
const RADIUS: f32 = 6.0;
/// The symbol column, from the left edge; the name starts right after it.
const SYMBOL_X: f32 = 4.0;
const SYMBOL_W: f32 = 24.0;
const NAME_X: f32 = SYMBOL_X + SYMBOL_W + 2.0;
/// The least room between a name and its folder, and after the folder.
const GAP: f32 = 12.0;
const PAD_RIGHT: f32 = 8.0;
/// The selected row: the accent, as the theme's lists show it.
const SELECTION_ALPHA: f32 = 0.33;
/// One turn of the wheel scrolls this many rows.
const WHEEL_ROWS: f64 = 3.0;
/// The scroll bar along the right edge.
const SCROLLBAR_W: f32 = 6.0;
const SCROLLBAR_INSET: f32 = 3.0;
const MIN_THUMB_H: f32 = 24.0;
/// A tooltip shows after the pointer rests on a symbol (or a shortened text) for this long.
const TOOLTIP_DELAY_S: f64 = 0.5;
const TIP_PAD_X: f32 = 8.0;
const TIP_PAD_Y: f32 = 5.0;
const TIP_MAX_W: f32 = 320.0;
/// What a tooltip keeps from the widget's edges.
const EDGE: f32 = 4.0;

/// Measures text: its width at a size, in logical units.
type Measure = fn(f32, &str) -> f32;

/// A change list. Cloning gives another handle to the same widget.
#[derive(Clone)]
pub struct RowsWidget {
    shared: Rc<Shared>,
}

/// A new, empty change list, without a parent yet: put it in an `Overlay` or a `Border` to fill
/// it. `on_select` gets the change the user selects (a click, a key, a right click); a
/// selection made in code is not reported.
pub fn new(on_select: impl FnMut(Change) + 'static) -> (RowsWidget, Widget) {
    let widget = custom::create();
    // For Up and Down: a press gives it the keyboard.
    widget.set_focusable(true);
    let shared = Rc::new(Shared {
        inner: RefCell::new(Inner::new(text_width)),
        on_select: RefCell::new(Box::new(on_select)),
        menu: Cell::new(None),
        widget,
    });
    let weak = Rc::downgrade(&shared);
    custom::on_paint(widget, {
        let weak = weak.clone();
        move |p, w, h| {
            if let Some(shared) = weak.upgrade() {
                shared.paint(p, w, h);
            }
        }
    });
    custom::on_event(widget, move |ev| {
        weak.upgrade().is_some_and(|shared| shared.event(ev))
    });
    (RowsWidget { shared }, widget)
}

impl RowsWidget {
    /// Shows `changes`, with the row `selected` selected. The scroll stays where it was, as far
    /// as the rows reach.
    pub fn set_rows(&self, changes: Vec<Change>, selected: Option<usize>) {
        self.shared.inner.borrow_mut().set_rows(changes, selected);
        self.shared.widget.invalidate();
    }

    /// Selects `row` (or nothing) and scrolls it into view.
    pub fn select(&self, row: Option<usize>) {
        self.shared.inner.borrow_mut().select(row);
        self.shared.widget.invalidate();
    }

    /// The selected change.
    pub fn selected(&self) -> Option<Change> {
        let inner = self.shared.inner.borrow();
        inner.selected.and_then(|i| inner.changes.get(i).cloned())
    }

    /// The row of the file at `path`.
    pub fn position(&self, path: &str) -> Option<usize> {
        let inner = self.shared.inner.borrow();
        inner.changes.iter().position(|change| change.path == path)
    }

    /// The changes shown.
    pub fn changes(&self) -> Vec<Change> {
        self.shared.inner.borrow().changes.clone()
    }

    /// Paints the list again (its tooltips are translated as they are painted).
    pub fn repaint(&self) {
        self.shared.widget.invalidate();
    }

    /// Opens `menu` in `window` on a right click on the list, after selecting the row under
    /// the pointer (which is reported like a click), so that the menu acts on that file.
    pub fn set_context_menu(&self, window: Window, menu: Widget) {
        self.shared.menu.set(Some((window, menu)));
    }
}

struct Shared {
    inner: RefCell<Inner>,
    on_select: RefCell<Box<dyn FnMut(Change)>>,
    /// The context menu and the window it opens in.
    menu: Cell<Option<(Window, Widget)>>,
    widget: Widget,
}

impl Shared {
    fn paint(&self, p: &mut Painter, w: f32, h: f32) {
        let Ok(mut inner) = self.inner.try_borrow_mut() else {
            return;
        };
        inner.resize(w, h);
        paint(p, &inner, w, h);
    }

    fn event(self: &Rc<Self>, ev: &Event) -> bool {
        let out = match self.inner.try_borrow_mut() {
            Ok(mut inner) => inner.event(ev),
            Err(_) => return false,
        };
        if out.focus {
            self.widget.focus();
        }
        if out.repaint {
            self.widget.invalidate();
        }
        if let Some(generation) = out.arm {
            self.arm_tooltip(generation);
        }
        if let Some(change) = out.report
            && let Ok(mut on_select) = self.on_select.try_borrow_mut()
        {
            on_select(change);
        }
        if let (Some((x, y)), Some((window, menu))) = (out.menu_at, self.menu.get()) {
            let (wx, wy, _, _) = self.widget.rect();
            window.open_menu(menu, wx + x, wy + y);
        }
        out.consumed
    }

    /// Shows the tooltip after a moment, if the pointer still rests on the same thing by then.
    fn arm_tooltip(self: &Rc<Self>, generation: u64) {
        let weak: Weak<Shared> = Rc::downgrade(self);
        task::timer(TOOLTIP_DELAY_S, false, move || {
            let Some(shared) = weak.upgrade() else { return };
            let due = shared
                .inner
                .try_borrow_mut()
                .is_ok_and(|mut inner| inner.tooltip_due(generation));
            if due {
                shared.widget.invalidate();
            }
        });
    }
}

/// What the widget does after an input, once its state is no longer borrowed.
#[derive(Debug, Default, PartialEq)]
struct Outcome {
    consumed: bool,
    repaint: bool,
    /// Take the keyboard, for Up and Down.
    focus: bool,
    /// The user selected this change.
    report: Option<Change>,
    /// Open the context menu here (in the widget's coordinates).
    menu_at: Option<(f32, f32)>,
    /// Show the tooltip after a moment, unless the hover changes before (its generation).
    arm: Option<u64>,
}

/// What a tooltip says, before it is translated.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Tip {
    /// The text of a key.
    Key(&'static str),
    /// "Renamed from {0}".
    RenamedFrom(String),
    /// A file's path or folder, as it is.
    Plain(String),
}

/// A rectangle: x, y, width, height.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect(f32, f32, f32, f32);

impl Rect {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.0 && x < self.0 + self.2 && y >= self.1 && y < self.1 + self.3
    }
}

/// Where a row's texts go.
#[derive(Clone, Debug, PartialEq)]
struct RowLook {
    name: String,
    /// The name is cut to fit (it ends in `…`).
    name_cut: bool,
    name_end: f32,
    /// The folder as it fits, right-aligned at `folder_x`.
    folder: String,
    folder_x: f32,
    folder_cut: bool,
}

/// Where the texts of the row of `path` go when the rows end at `right`: the name first, cut
/// only when it does not fit alone; the folder in what is left after it.
fn row_look(path: &str, right: f32, measure: Measure) -> RowLook {
    let (name, folder) = split_path(path);
    let room = f64::from(right - NAME_X);
    let shown = fit_end(name, room, |s| f64::from(measure(NAME_SIZE, s)));
    let name_end = NAME_X + measure(NAME_SIZE, &shown);
    let room = f64::from(right - name_end - GAP);
    let folder_shown = if folder.is_empty() || room <= 0.0 {
        String::new()
    } else {
        shorten_middle(folder, room, |s| f64::from(measure(FOLDER_SIZE, s)))
    };
    RowLook {
        name_cut: shown != name,
        name: shown,
        name_end,
        folder_x: right - measure(FOLDER_SIZE, &folder_shown),
        folder_cut: folder_shown != folder,
        folder: folder_shown,
    }
}

/// The color of a symbol.
fn symbol_color(s: Symbol) -> Rgba {
    match s {
        Symbol::Added => theme::ADDED,
        Symbol::Deleted => theme::REMOVED,
        Symbol::Changed => theme::UNSAVED,
        Symbol::Renamed => theme::RENAMED,
    }
}

/// The list's state: its rows, the selection, the scroll and what the pointer is on.
struct Inner {
    changes: Vec<Change>,
    selected: Option<usize>,
    model: RowsModel,
    /// The widget's size at the last paint.
    size: (f32, f32),
    pointer: Option<(f32, f32)>,
    /// The part of a row under the pointer, when it has a tooltip.
    tip_zone: Option<Zone>,
    /// Counts hover changes, so that a tooltip timer armed for an earlier one does nothing.
    hover_generation: u64,
    /// Where the tooltip shows once it is due: where the pointer was then.
    tooltip_at: Option<(f32, f32)>,
    /// While the scroll bar's thumb is dragged: the pointer's y and the scroll when it started.
    drag: Option<(f32, f64)>,
    /// Bring the selected row into view at the next paint, when the view's height is known.
    reveal: bool,
    measure: Measure,
}

/// What the pointer and the scroll make look different, to repaint only when it changes.
#[derive(Clone, Copy, PartialEq)]
struct Looks {
    scroll_y: f64,
    selected: Option<usize>,
    hovered_row: Option<usize>,
    tooltip: Option<(Zone, (f32, f32))>,
    hot_thumb: bool,
}

impl Inner {
    fn new(measure: Measure) -> Inner {
        Inner {
            changes: Vec::new(),
            selected: None,
            model: RowsModel {
                count: 0,
                row_h: f64::from(ROW_H),
                scroll_y: 0.0,
            },
            size: (0.0, 0.0),
            pointer: None,
            tip_zone: None,
            hover_generation: 0,
            tooltip_at: None,
            drag: None,
            reveal: false,
            measure,
        }
    }

    fn set_rows(&mut self, changes: Vec<Change>, selected: Option<usize>) {
        self.model.count = changes.len();
        self.changes = changes;
        self.selected = selected.filter(|&i| i < self.changes.len());
        self.drag = None;
        self.forget_hover();
        self.model.scroll_by(0.0, self.view_h());
    }

    fn select(&mut self, row: Option<usize>) {
        self.selected = row.filter(|&i| i < self.changes.len());
        self.reveal = true;
    }

    /// The widget is `w` by `h` now (it is being painted): the scroll stays within the rows,
    /// and a selection made in code comes into view.
    fn resize(&mut self, w: f32, h: f32) {
        self.size = (w, h);
        let view_h = self.view_h();
        if std::mem::take(&mut self.reveal)
            && let Some(row) = self.selected
        {
            self.model.ensure_visible(row, view_h);
        }
        self.model.scroll_by(0.0, view_h);
    }

    /// The height the rows have, inside the outline.
    fn view_h(&self) -> f64 {
        f64::from((self.size.1 - 2.0 * INSET).max(0.0))
    }

    /// The row at height `y` of the widget.
    fn row_at(&self, y: f32) -> Option<usize> {
        self.model.row_at(f64::from(y - INSET))
    }

    /// The top of row `i` in the widget, as painted now.
    fn row_y(&self, i: usize) -> f32 {
        INSET + (i as f64 * self.model.row_h - self.model.scroll_y) as f32
    }

    /// The scroll bar's track and thumb, when the rows do not all fit.
    fn scrollbar(&self) -> Option<(Rect, Rect)> {
        let view_h = self.view_h() as f32;
        let content_h = (self.model.count as f64 * self.model.row_h) as f32;
        if content_h <= view_h || view_h <= 2.0 * SCROLLBAR_INSET {
            return None;
        }
        let x = self.size.0 - INSET - SCROLLBAR_INSET - SCROLLBAR_W;
        let track = Rect(
            x,
            INSET + SCROLLBAR_INSET,
            SCROLLBAR_W,
            view_h - 2.0 * SCROLLBAR_INSET,
        );
        let thumb_h = (track.3 * view_h / content_h).clamp(MIN_THUMB_H.min(track.3), track.3);
        let travel = track.3 - thumb_h;
        let max = self.model.max_scroll(f64::from(view_h));
        let k = if max > 0.0 {
            (self.model.scroll_y / max) as f32
        } else {
            0.0
        };
        Some((track, Rect(x, track.1 + travel * k, SCROLLBAR_W, thumb_h)))
    }

    /// Whether (`x`, `y`) is on the scroll bar, which takes a little more than it paints.
    fn over_scrollbar(&self, x: f32, y: f32) -> bool {
        self.scrollbar().is_some_and(|(track, _)| {
            let reach = Rect(
                track.0 - SCROLLBAR_INSET,
                track.1 - SCROLLBAR_INSET,
                track.2 + 2.0 * SCROLLBAR_INSET,
                track.3 + 2.0 * SCROLLBAR_INSET,
            );
            reach.contains(x, y)
        })
    }

    /// Where the rows' texts end: before the scroll bar when there is one.
    fn right_edge(&self) -> f32 {
        let bar = if self.scrollbar().is_some() {
            SCROLLBAR_W + SCROLLBAR_INSET
        } else {
            0.0
        };
        self.size.0 - INSET - PAD_RIGHT - bar
    }

    fn look(&self, change: &Change) -> RowLook {
        row_look(&change.path, self.right_edge(), self.measure)
    }

    /// The row under the pointer, to light up.
    fn hovered_row(&self) -> Option<usize> {
        let (x, y) = self.pointer?;
        if self.over_scrollbar(x, y) {
            return None;
        }
        self.row_at(y)
    }

    /// The part of a row at (`x`, `y`) when it has a tooltip: a symbol always, a name or a
    /// folder only when it is shortened.
    fn tip_zone_at(&self, x: f32, y: f32) -> Option<Zone> {
        if self.over_scrollbar(x, y) {
            return None;
        }
        let change = self.changes.get(self.row_at(y)?)?;
        let look = self.look(change);
        let zone = self.model.zone_at(
            f64::from(x),
            f64::from(y - INSET),
            f64::from(SYMBOL_X + SYMBOL_W),
            f64::from(look.name_end),
        )?;
        let has_tip = match zone {
            Zone::Symbol(_) => true,
            Zone::Name(_) => look.name_cut,
            Zone::Folder(_) => look.folder_cut,
        };
        has_tip.then_some(zone)
    }

    fn looks(&self) -> Looks {
        let thumb = self.scrollbar().map(|(_, thumb)| thumb);
        let over_thumb =
            matches!((self.pointer, thumb), (Some((x, y)), Some(t)) if t.contains(x, y));
        Looks {
            scroll_y: self.model.scroll_y,
            selected: self.selected,
            hovered_row: self.hovered_row(),
            tooltip: self.tip_zone.zip(self.tooltip_at),
            hot_thumb: self.drag.is_some() || over_thumb,
        }
    }

    fn event(&mut self, ev: &Event) -> Outcome {
        let before = self.looks();
        let mut out = Outcome::default();
        match *ev {
            Event::MouseMove { x, y, .. } => {
                self.pointer = Some((x, y));
                if let Some((start_y, start_scroll)) = self.drag {
                    self.drag_thumb(start_y, start_scroll, y);
                }
                out.consumed = true;
            }
            Event::MouseDown {
                x,
                y,
                button: MouseButton::Left,
                ..
            } => {
                self.pointer = Some((x, y));
                self.tooltip_at = None;
                out.report = self.press(x, y);
                out.focus = true;
                out.consumed = true;
            }
            Event::MouseDown {
                x,
                y,
                button: MouseButton::Right,
                ..
            } => {
                // The row under the pointer first, so that the menu acts on it. Elsewhere the
                // selection stays, and the menu still opens (its other items act on the
                // project).
                self.pointer = Some((x, y));
                self.tooltip_at = None;
                out.report = self.row_at(y).and_then(|row| self.choose(row));
                out.menu_at = Some((x, y));
                out.focus = true;
                out.consumed = true;
            }
            Event::MouseUp { button, .. } => {
                if button == MouseButton::Left {
                    self.drag = None;
                }
                out.consumed = matches!(button, MouseButton::Left | MouseButton::Right);
            }
            Event::Wheel { x, y, wy, .. } => {
                self.pointer = Some((x, y));
                self.tooltip_at = None;
                let dy = -f64::from(wy) * WHEEL_ROWS * self.model.row_h;
                self.model.scroll_by(dy, self.view_h());
                out.consumed = true;
            }
            Event::KeyDown { key: Key::Up, .. } => {
                out.report = self.step(-1);
                out.consumed = true;
            }
            Event::KeyDown { key: Key::Down, .. } => {
                out.report = self.step(1);
                out.consumed = true;
            }
            Event::Leave => {
                self.pointer = None;
                self.drag = None;
            }
            _ => {}
        }
        // What the pointer rests on now: the rows may have moved under it.
        let zone = match self.pointer {
            Some((x, y)) if self.drag.is_none() => self.tip_zone_at(x, y),
            _ => None,
        };
        out.arm = self.set_hover(zone);
        out.repaint = self.looks() != before;
        out
    }

    /// A left press: on the thumb it starts a drag, elsewhere on the scroll bar it scrolls a
    /// page, and on a row it selects it.
    fn press(&mut self, x: f32, y: f32) -> Option<Change> {
        if let Some((_, thumb)) = self.scrollbar()
            && self.over_scrollbar(x, y)
        {
            if y >= thumb.1 && y < thumb.1 + thumb.3 {
                self.drag = Some((y, self.model.scroll_y));
            } else {
                let page = if y < thumb.1 { -1.0 } else { 1.0 } * self.view_h();
                self.model.scroll_by(page, self.view_h());
            }
            return None;
        }
        let row = self.row_at(y)?;
        self.choose(row)
    }

    /// The user selects `row`: it comes whole into view. Returns its change when the selection
    /// changed, to report it.
    fn choose(&mut self, row: usize) -> Option<Change> {
        self.model.ensure_visible(row, self.view_h());
        if self.selected == Some(row) {
            return None;
        }
        self.selected = Some(row);
        self.changes.get(row).cloned()
    }

    /// Up (`by` = -1) or Down (1): the row before or after the selected one, or the first.
    fn step(&mut self, by: isize) -> Option<Change> {
        let last = self.changes.len().checked_sub(1)?;
        let row = match self.selected {
            Some(i) => i.saturating_add_signed(by).min(last),
            None => 0,
        };
        self.choose(row)
    }

    /// Moves the scroll with the thumb: the pointer went from `start_y` to `y`.
    fn drag_thumb(&mut self, start_y: f32, start_scroll: f64, y: f32) {
        let Some((track, thumb)) = self.scrollbar() else {
            return;
        };
        let travel = f64::from(track.3 - thumb.3);
        if travel <= 0.0 {
            return;
        }
        let max = self.model.max_scroll(self.view_h());
        let target = start_scroll + f64::from(y - start_y) * max / travel;
        self.model
            .scroll_by(target - self.model.scroll_y, self.view_h());
    }

    /// Notes what the pointer rests on. Returns the generation to arm a tooltip for, when it
    /// came to rest on something new that has one.
    fn set_hover(&mut self, zone: Option<Zone>) -> Option<u64> {
        if zone == self.tip_zone {
            return None;
        }
        self.tip_zone = zone;
        self.tooltip_at = None;
        self.hover_generation = self.hover_generation.wrapping_add(1);
        zone.map(|_| self.hover_generation)
    }

    /// Forgets what the pointer was on (and its tooltip, even one still to come): the rows
    /// changed under it. The pointer's next move finds out.
    fn forget_hover(&mut self) {
        self.tip_zone = None;
        self.tooltip_at = None;
        self.hover_generation = self.hover_generation.wrapping_add(1);
    }

    /// The tooltip armed for `generation` is due: it shows, if the pointer still rests on the
    /// same thing. Returns whether it shows.
    fn tooltip_due(&mut self, generation: u64) -> bool {
        let still = self.hover_generation == generation && self.tip_zone.is_some();
        if still && self.drag.is_none() && self.tooltip_at.is_none() {
            self.tooltip_at = self.pointer;
        }
        still && self.tooltip_at.is_some()
    }

    /// What the tooltip on show says.
    fn tip(&self) -> Option<Tip> {
        self.tooltip_at?;
        Some(match self.tip_zone? {
            Zone::Symbol(row) => match &self.changes.get(row)?.kind {
                ChangeKind::Renamed { from } => Tip::RenamedFrom(from.clone()),
                kind => Tip::Key(tooltip_key(symbol(kind))),
            },
            Zone::Name(row) => Tip::Plain(self.changes.get(row)?.path.clone()),
            Zone::Folder(row) => {
                let path = &self.changes.get(row)?.path;
                Tip::Plain(split_path(path).1.to_owned())
            }
        })
    }
}

/// The whole list: its rows in view, the scroll bar, the outline and the tooltip.
fn paint(p: &mut Painter, inner: &Inner, w: f32, h: f32) {
    p.push_clip(0.0, 0.0, w, h);
    p.rect(0.0, 0.0, w, h, RADIUS, theme::FIELD);
    p.push_clip(INSET, INSET, w - 2.0 * INSET, h - 2.0 * INSET);
    let hovered = inner.hovered_row();
    let right = inner.right_edge();
    for i in inner.model.visible(inner.view_h()) {
        let y = inner.row_y(i);
        let background = if inner.selected == Some(i) {
            Some(with_alpha(theme::LINE, SELECTION_ALPHA))
        } else {
            (hovered == Some(i)).then_some(theme::SURFACE)
        };
        if let Some(color) = background {
            p.rect(INSET, y, w - 2.0 * INSET, ROW_H, 0.0, color);
        }
        paint_row(p, &inner.changes[i], y, right, inner.measure);
    }
    if let Some((_, thumb)) = inner.scrollbar() {
        let color = if inner.looks().hot_thumb {
            theme::MUTED_TEXT
        } else {
            theme::BORDER
        };
        p.rect(thumb.0, thumb.1, thumb.2, thumb.3, SCROLLBAR_W / 2.0, color);
    }
    p.pop_clip();
    p.rect_outline(0.0, 0.0, w, h, RADIUS, 1.0, theme::BORDER);
    if let (Some(tip), Some(at)) = (inner.tip(), inner.tooltip_at) {
        let text = match tip {
            Tip::Key(key) => tr(key),
            Tip::RenamedFrom(from) => tr_fmt("change.tip.renamed", &[&from]),
            Tip::Plain(text) => text,
        };
        paint_tooltip(p, &text, at, w, h);
    }
    p.pop_clip();
}

/// One row at `y`: the symbol in its color, the name, and the folder aligned at `right`.
fn paint_row(p: &mut Painter, change: &Change, y: f32, right: f32, measure: Measure) {
    let s = symbol(&change.kind);
    let text = symbol_text(s);
    let (sw, sh) = mono_text_size(SYMBOL_SIZE, text);
    let sx = SYMBOL_X + (SYMBOL_W - sw) / 2.0;
    p.mono_bold_text(
        SYMBOL_SIZE,
        sx,
        y + (ROW_H - sh) / 2.0,
        text,
        symbol_color(s),
    );
    let look = row_look(&change.path, right, measure);
    let name_y = y + (ROW_H - NAME_SIZE * LINE_HEIGHT) / 2.0;
    p.text(NAME_SIZE, NAME_X, name_y, &look.name, theme::TEXT);
    if !look.folder.is_empty() {
        // On the name's baseline.
        let folder_y = name_y + (NAME_SIZE - FOLDER_SIZE) * 0.95;
        p.text(
            FOLDER_SIZE,
            look.folder_x,
            folder_y,
            &look.folder,
            theme::MUTED_TEXT,
        );
    }
}

/// A tooltip near `at` (the pointer when it came due), below it or above when there is no room,
/// always inside the widget. Long texts wrap at spaces and between folders.
fn paint_tooltip(p: &mut Painter, text: &str, at: (f32, f32), w: f32, h: f32) {
    let max_w = (w - 2.0 * (EDGE + TIP_PAD_X)).min(TIP_MAX_W);
    let lines = wrap_text(text, f64::from(max_w), |s| {
        f64::from(text_width(TIP_SIZE, s))
    });
    if lines.is_empty() {
        return;
    }
    let text_w = lines
        .iter()
        .map(|line| text_width(TIP_SIZE, line))
        .fold(0.0, f32::max);
    let line_h = TIP_SIZE * LINE_HEIGHT;
    let (bw, bh) = (
        text_w + 2.0 * TIP_PAD_X,
        lines.len() as f32 * line_h + 2.0 * TIP_PAD_Y,
    );
    let (px, py) = at;
    let x = (px + 12.0).min(w - bw - EDGE).max(EDGE);
    let mut y = py + 20.0;
    if y + bh > h - EDGE {
        y = (py - bh - 6.0).max(EDGE);
    }
    p.rect(x, y, bw, bh, RADIUS, theme::SURFACE);
    p.rect_outline(x, y, bw, bh, RADIUS, 1.0, theme::BORDER);
    for (i, line) in lines.iter().enumerate() {
        let ly = y + TIP_PAD_Y + i as f32 * line_h;
        p.text(TIP_SIZE, x + TIP_PAD_X, ly, line, theme::TEXT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seven units of width per character, whatever the size.
    fn chars(_size: f32, s: &str) -> f32 {
        s.chars().count() as f32 * 7.0
    }

    fn change(path: &str, kind: ChangeKind) -> Change {
        Change {
            path: path.to_owned(),
            kind,
        }
    }

    /// A list 300 by 200 (8 rows of 24 fit, and a bit) of `changes`, the first one selected.
    fn inner_with(changes: Vec<Change>) -> Inner {
        let mut inner = Inner::new(chars);
        inner.set_rows(changes, Some(0));
        inner.resize(300.0, 200.0);
        inner
    }

    fn sample() -> Vec<Change> {
        vec![
            change("css/estilo.css", ChangeKind::Changed),
            change("rascunho.txt", ChangeKind::New),
            change("paginas/galeria.html", ChangeKind::Deleted),
            change(
                "paginas/quem-somos.html",
                ChangeKind::Renamed {
                    from: "paginas/sobre.html".to_owned(),
                },
            ),
            change(
                "src/app/componentes/formularios/campos/texto/campo.tsx",
                ChangeKind::New,
            ),
        ]
    }

    /// The middle of row `i` (as painted now), at `x`.
    fn at_row(inner: &Inner, i: usize, x: f32) -> (f32, f32) {
        (x, inner.row_y(i) + ROW_H / 2.0)
    }

    fn down(x: f32, y: f32, button: MouseButton) -> Event {
        Event::MouseDown {
            x,
            y,
            button,
            clicks: 1,
        }
    }

    fn moved(x: f32, y: f32) -> Event {
        Event::MouseMove {
            x,
            y,
            dx: 0.0,
            dy: 0.0,
        }
    }

    fn key(key: Key) -> Event {
        Event::KeyDown {
            key,
            ctrl: false,
            shift: false,
        }
    }

    #[test]
    fn a_click_selects_and_reports_once() {
        let mut inner = inner_with(sample());
        let (x, y) = at_row(&inner, 2, 100.0);
        let out = inner.event(&down(x, y, MouseButton::Left));
        assert_eq!(
            out.report.map(|c| c.path),
            Some("paginas/galeria.html".into())
        );
        assert!(out.focus && out.consumed && out.repaint);
        assert_eq!(inner.selected, Some(2));
        // The same row again: nothing new to report.
        assert_eq!(inner.event(&down(x, y, MouseButton::Left)).report, None);
        // Under the last row: nothing changes.
        let out = inner.event(&down(100.0, 190.0, MouseButton::Left));
        assert_eq!(out.report, None);
        assert_eq!(inner.selected, Some(2));
    }

    #[test]
    fn up_and_down_move_the_selection_and_keep_it_in_view() {
        let changes: Vec<Change> = (0..50)
            .map(|i| change(&format!("arquivo{i}.txt"), ChangeKind::New))
            .collect();
        let mut inner = inner_with(changes);
        assert_eq!(inner.event(&key(Key::Up)).report, None, "already the first");
        for i in 1..=12 {
            let out = inner.event(&key(Key::Down));
            assert_eq!(out.report.map(|c| c.path), Some(format!("arquivo{i}.txt")));
        }
        // Row 12 shows whole at the bottom of the 198-high view.
        assert_eq!(inner.selected, Some(12));
        assert_eq!(inner.model.scroll_y, 13.0 * 24.0 - 198.0);
        inner.event(&key(Key::Up));
        assert_eq!(inner.selected, Some(11));
        // No selection: Down picks the first row.
        inner.set_rows(inner.changes.clone(), None);
        assert_eq!(
            inner.event(&key(Key::Down)).report.map(|c| c.path),
            Some("arquivo0.txt".to_owned())
        );
        assert_eq!(inner.model.scroll_y, 0.0);
        // An empty list ignores them.
        inner.set_rows(Vec::new(), None);
        let out = inner.event(&key(Key::Down));
        assert_eq!((out.report, out.consumed), (None, true));
    }

    #[test]
    fn a_right_click_selects_the_row_then_opens_the_menu() {
        let mut inner = inner_with(sample());
        let (x, y) = at_row(&inner, 3, 60.0);
        let out = inner.event(&down(x, y, MouseButton::Right));
        assert_eq!(
            out.report.map(|c| c.path),
            Some("paginas/quem-somos.html".into())
        );
        assert_eq!(out.menu_at, Some((x, y)));
        assert_eq!(inner.selected, Some(3));
        // Again on the selected row: the menu, without a second report.
        let out = inner.event(&down(x, y, MouseButton::Right));
        assert_eq!((out.report, out.menu_at), (None, Some((x, y))));
        // Under the rows: the menu opens and the selection stays.
        let out = inner.event(&down(60.0, 190.0, MouseButton::Right));
        assert_eq!((out.report, out.menu_at), (None, Some((60.0, 190.0))));
        assert_eq!(inner.selected, Some(3));
        assert!(
            inner
                .event(&Event::MouseUp {
                    x: 60.0,
                    y: 190.0,
                    button: MouseButton::Right
                })
                .consumed
        );
    }

    #[test]
    fn resting_on_a_symbol_shows_what_it_means() {
        let mut inner = inner_with(sample());
        let (x, y) = at_row(&inner, 1, SYMBOL_X + SYMBOL_W / 2.0);
        let out = inner.event(&moved(x, y));
        let generation = out.arm.expect("a tooltip is armed");
        assert_eq!(inner.tip(), None, "not before the delay");
        assert!(inner.tooltip_due(generation));
        assert_eq!(inner.tip(), Some(Tip::Key("change.tip.new")));
        // Moving along the same symbol keeps it; the renamed row's says where from.
        assert_eq!(inner.event(&moved(x + 2.0, y)).arm, None);
        assert!(inner.tip().is_some());
        let (x, y) = at_row(&inner, 3, SYMBOL_X + 4.0);
        let generation = inner.event(&moved(x, y)).arm.expect("armed again");
        assert_eq!(inner.tip(), None, "the old one went away");
        assert!(inner.tooltip_due(generation));
        assert_eq!(
            inner.tip(),
            Some(Tip::RenamedFrom("paginas/sobre.html".to_owned()))
        );
        // A timer armed for an earlier hover does nothing.
        assert!(!inner.tooltip_due(generation - 1));
        // A press hides it; the wheel too; leaving forgets it.
        inner.event(&down(x, y, MouseButton::Left));
        assert_eq!(inner.tip(), None);
        assert!(!inner.event(&Event::Leave).consumed);
        assert_eq!(inner.tip_zone, None);
    }

    #[test]
    fn a_shortened_folder_shows_whole_on_hover() {
        let mut inner = inner_with(sample());
        // Row 4: "campo.tsx" and a folder too long for the 300 px row.
        let look = inner.look(&inner.changes[4]);
        assert!(look.folder_cut && !look.name_cut, "{look:?}");
        assert!(
            look.folder.contains('…') && look.folder.ends_with('/'),
            "{look:?}"
        );
        let (x, y) = at_row(&inner, 4, look.folder_x + 5.0);
        let generation = inner.event(&moved(x, y)).arm.expect("armed");
        assert!(inner.tooltip_due(generation));
        assert_eq!(
            inner.tip(),
            Some(Tip::Plain(
                "src/app/componentes/formularios/campos/texto/".to_owned()
            ))
        );
        // A folder that shows whole, or a name that does, has nothing more to tell.
        let look = inner.look(&inner.changes[0]);
        assert!(!look.folder_cut && !look.name_cut);
        let (x, y) = at_row(&inner, 0, look.folder_x + 2.0);
        assert_eq!(inner.event(&moved(x, y)).arm, None);
        let (x, y) = at_row(&inner, 0, NAME_X + 3.0);
        assert_eq!(inner.event(&moved(x, y)).arm, None);
    }

    #[test]
    fn a_cut_name_shows_the_whole_path_on_hover() {
        let long = "uma-pasta/um-nome-de-arquivo-comprido-demais-para-a-lista.tsx";
        let mut inner = inner_with(vec![change(long, ChangeKind::Changed)]);
        let look = inner.look(&inner.changes[0]);
        assert!(look.name_cut && look.name.ends_with('…'), "{look:?}");
        assert_eq!(look.folder, "", "no room left for the folder");
        let (x, y) = at_row(&inner, 0, NAME_X + 10.0);
        let generation = inner.event(&moved(x, y)).arm.expect("armed");
        assert!(inner.tooltip_due(generation));
        assert_eq!(inner.tip(), Some(Tip::Plain(long.to_owned())));
    }

    #[test]
    fn the_wheel_and_the_scroll_bar_scroll() {
        let changes: Vec<Change> = (0..1000)
            .map(|i| change(&format!("node_modules/pacote{i}/index.js"), ChangeKind::New))
            .collect();
        let mut inner = inner_with(changes);
        let max = 1000.0 * 24.0 - 198.0;
        // One notch down is three rows; up past the top stops there.
        let out = inner.event(&Event::Wheel {
            x: 50.0,
            y: 50.0,
            wx: 0.0,
            wy: -1.0,
            ctrl: false,
            shift: false,
        });
        assert!(out.consumed && out.repaint);
        assert_eq!(inner.model.scroll_y, 72.0);
        inner.event(&Event::Wheel {
            x: 50.0,
            y: 50.0,
            wx: 0.0,
            wy: 10.0,
            ctrl: false,
            shift: false,
        });
        assert_eq!(inner.model.scroll_y, 0.0);
        // A press on the track below the thumb scrolls a page.
        let (track, thumb) = inner.scrollbar().expect("a scroll bar");
        let x = track.0 + 1.0;
        inner.event(&down(x, thumb.1 + thumb.3 + 20.0, MouseButton::Left));
        assert_eq!(inner.model.scroll_y, 198.0);
        assert_eq!(inner.selected, Some(0), "the scroll bar selects nothing");
        // Dragging the thumb to the bottom of the track reaches the last row.
        let (_, thumb) = inner.scrollbar().unwrap();
        let grab = thumb.1 + 2.0;
        inner.event(&down(x, grab, MouseButton::Left));
        inner.event(&moved(x, grab + track.3));
        assert_eq!(inner.model.scroll_y, max);
        let (_, thumb) = inner.scrollbar().unwrap();
        assert!((thumb.1 + thumb.3 - (track.1 + track.3)).abs() < 0.01);
        inner.event(&Event::MouseUp {
            x,
            y: grab,
            button: MouseButton::Left,
        });
        assert_eq!(inner.drag, None);
        // Only the rows in view are painted.
        let visible = inner.model.visible(inner.view_h());
        assert!(visible.len() <= 10, "{visible:?}");
        assert_eq!(visible.end, 1000);
    }

    #[test]
    fn new_rows_keep_the_scroll_within_them_and_forget_the_hover() {
        let changes: Vec<Change> = (0..100)
            .map(|i| change(&format!("a{i}.txt"), ChangeKind::Changed))
            .collect();
        let mut inner = inner_with(changes.clone());
        inner.model.scroll_by(2000.0, inner.view_h());
        let (x, y) = at_row(&inner, 85, SYMBOL_X + 2.0);
        assert!(inner.event(&moved(x, y)).arm.is_some());
        inner.set_rows(changes[..20].to_vec(), Some(19));
        assert_eq!(inner.model.scroll_y, 20.0 * 24.0 - 198.0);
        assert_eq!(inner.tip_zone, None);
        // A selection made in code comes into view at the next paint.
        inner.set_rows(changes.clone(), Some(0));
        inner.select(Some(60));
        inner.resize(300.0, 200.0);
        assert_eq!(inner.model.scroll_y, 61.0 * 24.0 - 198.0);
        // Out of range: nothing is selected.
        inner.set_rows(changes[..3].to_vec(), Some(7));
        assert_eq!(inner.selected, None);
    }
}
