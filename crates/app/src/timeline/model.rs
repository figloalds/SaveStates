//! Pure model of the timeline: saves placed by time on a day grid, one lane per timeline, a
//! pannable and zoomable view that also scrolls the lanes, grouping, hit testing, the temporary
//! zoom, paging and input handling. Nothing here draws, calls Raven or reads the clock; the
//! widget paints this state and feeds it events.
//!
//! Each lane holds its timeline's saves, oldest on the left, each at its local commit time (see
//! [`super::axis`]); a lane that branches off another starts no further left than the save it
//! branches off. "Agora", for the unsaved changes, sits at the current time, at least
//! [`NOW_MIN_GAP_PX`] right of "Você está aqui", on the current lane or in the pending slot
//! under it (spec §6.5). Saves whose balls would overlap share a group ball, lane by lane (see
//! [`super::layout`]). Clicking a group zooms in on it temporarily: Esc goes back to the view
//! from before, and any other pan or zoom keeps the new view.
//!
//! Vertically, under a header row of [`HEADER_PX`], come the slots: the lanes in display order,
//! each [`LANE_PX`] tall with its line [`LINE_OFFSET_PX`] below its top, and the pending slot
//! right after the lane it continues from. When they don't fit, they scroll.

use std::ops::Range;

use chrono::{DateTime, FixedOffset, NaiveDateTime};
use savestates_core::SaveState;
use savestates_ui_raven::{Event, MouseButton, Rgba};

use super::axis::{self, Axis, DEFAULT_DAY_PX, Days, NOW_MIN_GAP_PX, ToLocal, View, place_saves};
use super::layout::{Hit, Item, hit_test, layout, layout_range, splits_at};
use crate::theme;

/// The grid's header row, with the day and hour labels, above the lanes.
pub const HEADER_PX: f64 = 24.0;
/// The height of a lane, and of the pending slot.
pub const LANE_PX: f64 = 76.0;
/// How far below its lane's top a line sits.
pub const LINE_OFFSET_PX: f64 = 32.0;

/// "Agora" sits this far from the right edge on open and after jumping to the end.
const NOW_MARGIN_PX: f64 = NOW_MIN_GAP_PX + 24.0;
/// A press and release that moved less than this (in pixels, in total) is a click, not a pan.
const CLICK_SLOP_PX: f64 = 4.0;
/// How far one wheel line pans, in screen pixels.
const WHEEL_PAN_PX: f64 = 60.0;
/// How much one wheel line zooms.
const WHEEL_ZOOM_STEP: f64 = 1.15;
/// A clicked group fills this part of the width, in the middle.
const GROUP_FILL: f64 = 0.6;
/// A lane's name box: this tall from the lane's top, and this much wider than the name.
pub const NAME_BOX_H_PX: f64 = 18.0;
pub const NAME_BOX_PAD_PX: f64 = 24.0;
/// When the lanes don't fit, a press this close to the right edge drags the scroll bar.
const SCROLL_BAR_PX: f64 = 10.0;
/// A lane's first save sits at least this long after the save it branches off.
const ONE_SECOND: Days = 1.0 / 86_400.0;

/// What is selected on the timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    /// "Agora": the unsaved changes.
    Now,
    /// The save at this index in [`TimelineData::saves`].
    Station(usize),
}

/// One lane: a timeline, or the save the project is on when it is on no timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct LaneData {
    /// The name shown at the lane's top-left.
    pub label: String,
    /// The git branch, `None` for the detached HEAD's lane.
    pub branch: Option<String>,
    pub color: Rgba,
    /// The lane's saves in [`TimelineData::saves`], oldest first.
    pub saves: Range<usize>,
    /// The save it branches off, in [`TimelineData::saves`] (in a lane before this one).
    pub fork: Option<usize>,
    /// The lane's oldest save has an older one that is not loaded yet.
    pub continues_left: bool,
    /// The current timeline is archived (only the current one is shown when archived).
    pub archived: bool,
}

#[derive(Clone, Debug)]
pub struct TimelineData {
    /// Lane by lane in display order, each lane oldest first.
    pub saves: Vec<SaveState>,
    /// In display order: the main line first.
    pub lanes: Vec<LaneData>,
    /// Branches without a save of their own: the save each points at, and its name.
    pub tags: Vec<(usize, String)>,
    /// "Você está aqui": the save the project is on. `None` without saves.
    pub here: Option<usize>,
    /// The current timeline's lane; `None` when it is pending (or there is none).
    pub current_lane: Option<usize>,
    /// The lane the pending slot sits right after; `None` when there is no pending slot.
    pub pending_after: Option<usize>,
    /// Whether older saves exist beyond the ones loaded.
    pub more_available: bool,
    /// How many files have unsaved changes.
    pub unsaved: usize,
    pub selection: Selection,
}

impl TimelineData {
    /// One unnamed lane of `saves_oldest_first`, the project at the newest: a single timeline,
    /// as wave 1 showed it.
    pub fn one_lane(
        saves_oldest_first: Vec<SaveState>,
        more_available: bool,
        unsaved: usize,
    ) -> TimelineData {
        let count = saves_oldest_first.len();
        TimelineData {
            saves: saves_oldest_first,
            lanes: vec![LaneData {
                label: String::new(),
                branch: None,
                color: theme::LINE,
                saves: 0..count,
                fork: None,
                continues_left: more_available,
                archived: false,
            }],
            tags: Vec::new(),
            here: count.checked_sub(1),
            current_lane: Some(0),
            pending_after: None,
            more_available,
            unsaved,
            selection: Selection::Now,
        }
    }

    /// The index of the save the project is on. `None` without saves.
    pub fn you_are_here(&self) -> Option<usize> {
        self.here
    }
}

/// `label` cut to at most `max_chars` characters, ending in `…` when something was cut.
pub fn truncate_label(label: &str, max_chars: usize) -> String {
    if label.chars().count() <= max_chars {
        return label.to_owned();
    }
    if max_chars == 0 {
        return String::new();
    }
    let kept: String = label.chars().take(max_chars - 1).collect();
    format!("{}…", kept.trim_end())
}

/// What the owner of a [`TimelineState`] has to do after an event.
#[derive(Clone, Debug, PartialEq)]
pub enum TimelineAction {
    /// A save or "Agora" was clicked. `data.selection` already holds it; repaint.
    Select(Selection),
    /// The user double-clicked "Agora". The view is already at the end.
    JumpToEnd,
    /// Load the page of saves before the oldest one loaded.
    RequestOlder,
    /// The view or the temporary zoom changed; repaint.
    Redraw,
    /// The view jumped from `from` to where it is now: animate the move.
    Animate { from: View },
    /// A group that cannot split any further was clicked: list its saves.
    OpenGroupList(Range<usize>),
    /// The right button was let go over a lane's name, at (`x`, `y`): open its menu.
    LaneMenu { lane: usize, x: f32, y: f32 },
    /// A lane's name was double-clicked: rename its timeline.
    Rename(usize),
}

/// The temporary zoom a group click starts: while it lasts, the view Esc goes back to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TempZoom {
    return_to: Option<View>,
}

impl TempZoom {
    /// Starts a temporary zoom away from `current`. When one is already active, Esc still goes
    /// back to where it started.
    pub fn begin(&mut self, current: View) {
        self.return_to.get_or_insert(current);
    }

    /// Ends the temporary zoom, giving the view to go back to; `None` when none was active.
    pub fn escape(&mut self) -> Option<View> {
        self.return_to.take()
    }

    /// Ends the temporary zoom, keeping the view where it is.
    pub fn accept(&mut self) {
        self.return_to = None;
    }

    pub fn active(&self) -> bool {
        self.return_to.is_some()
    }
}

/// What the left button, held down, is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Press {
    /// Pressed on the lanes: a click, or a pan once it moved [`CLICK_SLOP_PX`]. How far it has
    /// moved so far.
    Pan(f64),
    /// Dragging the scroll bar.
    ScrollBar,
}

#[derive(Clone, Debug)]
pub struct TimelineState {
    pub data: TimelineData,
    pub view: View,
    pub axis: Axis,
    pub temp: TempZoom,
    /// The world x of each save in `data.saves`.
    xs: Vec<Days>,
    /// The world x of the current time.
    now_x: Days,
    /// While the left button is held.
    press: Option<Press>,
    to_local: ToLocal,
    /// The current local time, which placing the saves needs: a save dated later sits at it.
    now_local: NaiveDateTime,
    /// The world x of the oldest save whose date could be read, in any lane (of the oldest
    /// save when none has one). Paging is judged by it.
    oldest_x: Option<Days>,
    /// The widget's height.
    height: f64,
    /// How far the lanes are scrolled up, in pixels.
    scroll_y: f64,
    /// The width of each lane's name, as the widget measures it.
    label_widths: Vec<f64>,
}

impl TimelineState {
    /// A state showing `data` in a widget `width` by `height` px, at the default zoom, with
    /// "Agora" near the right edge and the lanes scrolled to the top. The axis starts on the
    /// day of `now_local`.
    pub fn new(
        data: TimelineData,
        width: f64,
        height: f64,
        to_local: ToLocal,
        now_local: NaiveDateTime,
    ) -> Self {
        let axis = Axis {
            origin: now_local.date(),
        };
        let mut state = TimelineState {
            data,
            view: View {
                day_px: DEFAULT_DAY_PX,
                left: 0.0,
                width,
            },
            axis,
            temp: TempZoom::default(),
            xs: Vec::new(),
            now_x: axis.x_of(now_local),
            press: None,
            to_local,
            now_local,
            oldest_x: None,
            height,
            scroll_y: 0.0,
            label_widths: Vec::new(),
        };
        state.place_saves();
        state.put_now_at_end();
        state
    }

    /// Shows `data` instead, keeping the view and the scroll (as far as the new lanes allow),
    /// with "Agora" moved to `now_local`. New saves, older pages and other timelines all come
    /// this way.
    pub fn replace_data(&mut self, data: TimelineData, now_local: NaiveDateTime) {
        self.data = data;
        self.set_now(now_local);
        self.clamp_scroll();
    }

    /// Moves "Agora" to `now_local` (and a save dated after it along with it).
    pub fn set_now(&mut self, now_local: NaiveDateTime) {
        self.now_local = now_local;
        self.now_x = self.axis.x_of(now_local);
        self.place_saves();
    }

    /// Follows the widget's size, keeping what is at the right edge, where "Agora" usually is,
    /// in place. The view a temporary zoom goes back to follows too. The scroll stays within
    /// the lanes.
    pub fn set_size(&mut self, width: f64, height: f64) {
        keep_right_edge(&mut self.view, width);
        if let Some(view) = &mut self.temp.return_to {
            keep_right_edge(view, width);
        }
        self.height = height;
        self.clamp_scroll();
    }

    /// The width of each lane's name, in lane order: the widget measures them, and a lane's
    /// name box is that much plus some padding.
    pub fn set_label_widths(&mut self, widths: Vec<f64>) {
        self.label_widths = widths;
    }

    /// How far the lanes are scrolled up, in pixels.
    pub fn scroll_y(&self) -> f64 {
        self.scroll_y
    }

    /// The lane save `i` is drawn in.
    pub fn lane_of(&self, i: usize) -> Option<usize> {
        let lanes = &self.data.lanes;
        // Lanes hold consecutive ranges, in order.
        let k = lanes.partition_point(|lane| lane.saves.end <= i);
        lanes
            .get(k)
            .filter(|lane| lane.saves.contains(&i))
            .map(|_| k)
    }

    /// How many slots there are: the lanes, plus the pending slot when there is one.
    pub fn slot_count(&self) -> usize {
        self.data.lanes.len() + usize::from(self.data.pending_after.is_some())
    }

    /// The y of `lane`'s line, in widget coordinates.
    pub fn lane_line_y(&self, lane: usize) -> f64 {
        self.slot_line_y(self.slot_of_lane(lane))
    }

    /// The y "Agora" sits at: in the pending slot when there is one, else on the current lane,
    /// else on the first slot.
    pub fn now_line_y(&self) -> f64 {
        self.slot_line_y(self.now_slot())
    }

    /// The balls to draw: the saves near the view, grouped lane by lane. "Você está aqui" and
    /// the selected save keep their own balls.
    pub fn items(&self) -> Vec<Item> {
        let pinned = self.pinned();
        (0..self.data.lanes.len())
            .flat_map(|lane| self.lane_items(lane, &pinned))
            .collect()
    }

    /// The screen x of "Agora": at its time, or [`NOW_MIN_GAP_PX`] right of "Você está aqui",
    /// whichever is further right.
    pub fn now_screen_x(&self) -> f64 {
        let here = self.data.you_are_here().and_then(|i| self.xs.get(i));
        let here_x = here.map(|&x| self.view.world_to_screen(x));
        axis::now_screen_x(&self.view, self.now_x, here_x)
    }

    /// The world x of each save, in the order of `data.saves`: increasing within each lane.
    pub fn xs(&self) -> &[Days] {
        &self.xs
    }

    /// The local wall-clock time of `t`, as the saves are placed.
    pub fn local(&self, t: DateTime<FixedOffset>) -> NaiveDateTime {
        (self.to_local)(t)
    }

    /// The current local time, where "Agora" is.
    pub fn now_local(&self) -> NaiveDateTime {
        self.now_local
    }

    /// What the pointer at (`x`, `y`), in widget coordinates, is on: a lane's name, or what is
    /// in the slot under it. "Você está aqui" and the selected save are painted on top, so they
    /// win anywhere inside their painted balls.
    pub fn hit_at(&self, x: f64, y: f64) -> Option<Hit> {
        if let Some(lane) = self.lane_name_at(x, y) {
            return Some(Hit::LaneName(lane));
        }
        let slot = self.slot_at(y)?;
        // Out of reach anywhere but in its own slot.
        let now_x = if slot == self.now_slot() {
            self.now_screen_x()
        } else {
            f64::INFINITY
        };
        let pinned = self.pinned();
        let items = match self.lane_in_slot(slot) {
            Some(lane) => self.lane_items(lane, &pinned),
            None => Vec::new(),
        };
        hit_test(&items, &pinned, now_x, x, y, self.slot_line_y(slot))
    }

    /// Whether zooming in on the group of `members` (as a click on it does) splits it into more
    /// than one ball. When not, a click lists its saves.
    pub fn group_splits(&self, members: Range<usize>) -> bool {
        self.group_target(members.clone())
            .is_some_and(|target| splits_at(&target, &self.xs, members))
    }

    /// The view a click on the group of `members` zooms to: its saves fill the middle of the
    /// width. `None` for no saves.
    fn group_target(&self, members: Range<usize>) -> Option<View> {
        let first = self.xs.get(members.start)?;
        let last = self.xs.get(members.end.checked_sub(1)?)?;
        Some(self.view.fit(*first, *last, GROUP_FILL))
    }

    /// Scrolls so that "Agora" sits near the right edge, keeping the zoom, and its slot is in
    /// view. This ends the temporary zoom.
    pub fn jump_to_end(&mut self) {
        self.temp.accept();
        self.put_now_at_end();
        self.reveal_now_slot();
    }

    /// The view a tab's timeline opens with, the first time it shows: "Agora" near the right
    /// edge, at the zoom it has, with its slot in view. When that leaves "Você está aqui"
    /// off-screen (the last save was long ago), the two fill the middle of the width instead,
    /// as a clicked group does, as far as the zoom range allows: when even the widest zoom
    /// can't show both, "Agora" stays near the right edge.
    pub fn first_view(&mut self) {
        self.jump_to_end();
        let width = self.view.width;
        let Some(&here) = self.data.you_are_here().and_then(|i| self.xs.get(i)) else {
            return;
        };
        if width <= 0.0 || (0.0..=width).contains(&self.view.world_to_screen(here)) {
            return;
        }
        self.view = self.view.fit(here, self.now_x, GROUP_FILL);
        if self.now_screen_x() > width - NOW_MARGIN_PX {
            self.put_now_at_end();
        }
    }

    fn put_now_at_end(&mut self) {
        let dx = self.view.width - NOW_MARGIN_PX - self.now_screen_x();
        self.view.pan_by(dx);
    }

    /// Scrolls as little as needed for "Agora"'s whole slot to show under the header.
    fn reveal_now_slot(&mut self) {
        let room = self.height - HEADER_PX;
        if room <= 0.0 {
            return;
        }
        let top = self.now_slot() as f64 * LANE_PX;
        let bottom = top + LANE_PX;
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if bottom > self.scroll_y + room {
            self.scroll_y = bottom - room;
        }
        self.clamp_scroll();
    }

    /// Places every lane's saves by time, lanes in display order: a lane that branches off
    /// another starts no further left than its fork save, which an earlier lane holds.
    fn place_saves(&mut self) {
        let saves = &self.data.saves;
        let mut xs = vec![self.now_x; saves.len()];
        let (mut oldest_dated, mut oldest): (Option<Days>, Option<Days>) = (None, None);
        for lane in &self.data.lanes {
            let range = clamp_range(&lane.saves, saves.len());
            if range.is_empty() {
                continue;
            }
            let placed = place_saves(
                &self.axis,
                &saves[range.clone()],
                self.to_local,
                self.now_local,
            );
            let mut floor = lane
                .fork
                .filter(|&fork| fork < range.start)
                .map(|fork| xs[fork] + ONE_SECOND);
            for (i, p) in range.clone().zip(placed) {
                let x = floor.map_or(p.x, |floor| p.x.max(floor));
                if floor.is_some() {
                    floor = Some(x + ONE_SECOND);
                }
                xs[i] = x;
                let min = |old: Option<Days>| Some(old.map_or(x, |old| old.min(x)));
                oldest = min(oldest);
                if p.dated {
                    oldest_dated = min(oldest_dated);
                }
            }
        }
        self.xs = xs;
        self.oldest_x = oldest_dated.or(oldest);
    }

    /// The balls of `lane`'s saves near the view, laid out on their own.
    fn lane_items(&self, lane: usize, pinned: &[usize]) -> Vec<Item> {
        let Some(lane) = self.data.lanes.get(lane) else {
            return Vec::new();
        };
        let range = clamp_range(&lane.saves, self.xs.len());
        let near = layout_range(&self.view, &self.xs[range.clone()]);
        let start = range.start;
        layout(
            &self.view,
            &self.xs,
            near.start + start..near.end + start,
            pinned,
        )
    }

    /// The saves that never join a group: "Você está aqui" and the selected save.
    fn pinned(&self) -> Vec<usize> {
        let mut pinned: Vec<usize> = self
            .data
            .you_are_here()
            .filter(|&i| i < self.xs.len())
            .into_iter()
            .collect();
        if let Selection::Station(i) = self.data.selection
            && i < self.xs.len()
            && !pinned.contains(&i)
        {
            pinned.push(i);
        }
        pinned
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hit_at(f64::from(x), f64::from(y))
    }

    /// The slot `lane` is drawn in: the pending slot, when there is one, comes right after
    /// the lane it continues from.
    pub fn slot_of_lane(&self, lane: usize) -> usize {
        lane + usize::from(self.data.pending_after.is_some_and(|after| lane > after))
    }

    /// The lane in `slot`; `None` for the pending slot.
    fn lane_in_slot(&self, slot: usize) -> Option<usize> {
        let lane = match self.data.pending_after {
            Some(after) if slot == after + 1 => return None,
            Some(after) if slot > after + 1 => slot - 1,
            _ => slot,
        };
        (lane < self.data.lanes.len()).then_some(lane)
    }

    /// The pending slot, when there is one: right after the lane it continues from.
    pub fn pending_slot(&self) -> Option<usize> {
        self.data.pending_after.map(|after| after + 1)
    }

    /// The slot "Agora" sits in.
    pub fn now_slot(&self) -> usize {
        self.pending_slot()
            .or_else(|| self.data.current_lane.map(|lane| self.slot_of_lane(lane)))
            .unwrap_or(0)
    }

    /// How many slots are drawn: at least one, where "Agora" sits even without lanes.
    pub fn shown_slots(&self) -> usize {
        self.slot_count().max(1)
    }

    /// The top of `slot`, in widget coordinates.
    pub fn slot_top(&self, slot: usize) -> f64 {
        HEADER_PX + slot as f64 * LANE_PX - self.scroll_y
    }

    fn slot_line_y(&self, slot: usize) -> f64 {
        self.slot_top(slot) + LINE_OFFSET_PX
    }

    /// The slot at widget y `y`, if any: the header row and below the last slot hold none.
    fn slot_at(&self, y: f64) -> Option<usize> {
        if y.is_nan() || y < HEADER_PX {
            return None;
        }
        let row = ((y + self.scroll_y - HEADER_PX) / LANE_PX).floor();
        let slot = row as usize;
        (row >= 0.0 && slot < self.shown_slots()).then_some(slot)
    }

    /// The lane whose name box is at (`x`, `y`): at its top-left, as wide as its name plus
    /// [`NAME_BOX_PAD_PX`], [`NAME_BOX_H_PX`] tall. Not under the header.
    fn lane_name_at(&self, x: f64, y: f64) -> Option<usize> {
        let lane = self.lane_in_slot(self.slot_at(y)?)?;
        let top = self.slot_top(self.slot_of_lane(lane));
        let width = self.label_widths.get(lane).copied().unwrap_or(0.0) + NAME_BOX_PAD_PX;
        (y < top + NAME_BOX_H_PX && (0.0..width).contains(&x)).then_some(lane)
    }

    /// The height of the header and every slot.
    pub fn content_height(&self) -> f64 {
        HEADER_PX + self.shown_slots() as f64 * LANE_PX
    }

    fn max_scroll(&self) -> f64 {
        (self.content_height() - self.height).max(0.0)
    }

    /// Scrolls the lanes up by `dy` pixels (down for a negative `dy`), within the lanes.
    fn scroll_by(&mut self, dy: f64) {
        if dy.is_finite() {
            self.scroll_y += dy;
            self.clamp_scroll();
        }
    }

    fn clamp_scroll(&mut self) {
        self.scroll_y = self.scroll_y.clamp(0.0, self.max_scroll());
    }

    /// Whether a press at widget x `x` takes the scroll bar: the lanes don't fit, and it is
    /// within [`SCROLL_BAR_PX`] of the right edge.
    fn on_scroll_bar(&self, x: f64) -> bool {
        self.max_scroll() > 0.0 && x >= self.view.width - SCROLL_BAR_PX
    }
}

/// `range` within `0..len`.
fn clamp_range(range: &Range<usize>, len: usize) -> Range<usize> {
    let end = range.end.min(len);
    range.start.min(end)..end
}

/// `view` at `width`, with the same world x at its right edge.
fn keep_right_edge(view: &mut View, width: f64) {
    view.left += (view.width - width) / view.day_px;
    view.width = width;
}

/// Whether the view is close enough to the oldest loaded save to load the page before it: more
/// saves exist, and the view's left edge is less than one screen width right of the oldest
/// loaded save with a readable date, in any lane (the oldest save when none has one).
pub fn needs_older_page(s: &TimelineState) -> bool {
    s.data.more_available
        && s.oldest_x
            .is_some_and(|x| s.view.world_to_screen(x) > -s.view.width)
}

/// Applies an input event to the state and says what the owner has to do about it.
///
/// - The wheel zooms around the pointer, with or without Ctrl. Shift+wheel pans, and so does a
///   horizontal wheel.
/// - Dragging with the left button pans, both ways: sideways in time, up and down through the
///   lanes. A press and release that moved less than 4 px in all is a click. When the lanes
///   don't fit, a press at the right edge drags the scroll bar instead.
/// - A click on a save or "Agora" selects it. A click on a group zooms in on it temporarily, or
///   asks for its list when zooming in would not split it. A double click on "Agora" jumps to
///   the end; one on a lane's name renames it. The right button let go over a lane's name
///   opens its menu.
/// - Wheel turns and drags end the temporary zoom; selecting does not.
/// - A change of the view, the scroll or the temporary zoom gives `Redraw`, and a change of the
///   view gives `RequestOlder` when it needs an older page.
pub fn handle_event(s: &mut TimelineState, ev: &Event) -> Vec<TimelineAction> {
    let before = (s.view, s.temp, s.scroll_y);
    let mut actions = Vec::new();
    match *ev {
        Event::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            clicks,
        } => {
            s.press = None;
            if clicks == 1 {
                s.press = Some(if s.on_scroll_bar(f64::from(x)) {
                    Press::ScrollBar
                } else {
                    Press::Pan(0.0)
                });
            } else if clicks == 2 {
                match s.hit(x, y) {
                    Some(Hit::Now) => {
                        s.jump_to_end();
                        actions.extend([TimelineAction::JumpToEnd, TimelineAction::Redraw]);
                    }
                    Some(Hit::LaneName(lane)) => actions.push(TimelineAction::Rename(lane)),
                    _ => {}
                }
            }
        }
        Event::MouseMove { dx, dy, .. } => {
            let (dx, dy) = (f64::from(dx), f64::from(dy));
            match s.press {
                Some(Press::Pan(moved)) => {
                    let moved = moved + dx.hypot(dy);
                    s.press = Some(Press::Pan(moved));
                    // Under the click slop it may still be a click (a peek keeps the temporary
                    // zoom).
                    if moved >= CLICK_SLOP_PX {
                        s.temp.accept();
                    }
                    pan(&mut s.view, dx);
                    s.scroll_by(-dy);
                }
                Some(Press::ScrollBar) if s.height > 0.0 => {
                    // The thumb is the view's share of the content: the lanes move that much
                    // further than it.
                    s.scroll_by(dy * s.content_height() / s.height);
                }
                _ => {}
            }
        }
        Event::MouseUp {
            x,
            y,
            button: MouseButton::Left,
        } => {
            if let Some(Press::Pan(moved)) = s.press.take()
                && moved < CLICK_SLOP_PX
                && let Some(hit) = s.hit(x, y)
            {
                actions = click(s, hit);
            }
        }
        Event::MouseUp {
            x,
            y,
            button: MouseButton::Right,
        } => {
            if let Some(Hit::LaneName(lane)) = s.hit(x, y) {
                actions.push(TimelineAction::LaneMenu { lane, x, y });
            }
        }
        Event::Wheel {
            x, wx, wy, shift, ..
        } => {
            let (x, wx, wy) = (f64::from(x), f64::from(wx), f64::from(wy));
            if shift {
                pan(&mut s.view, wy * WHEEL_PAN_PX);
            } else {
                zoom(&mut s.view, wy, x);
            }
            pan(&mut s.view, wx * WHEEL_PAN_PX);
            s.temp.accept();
        }
        Event::Leave => s.press = None,
        _ => {}
    }
    finish(s, before, actions)
}

/// Esc: when a temporary zoom is active, goes back to the view from before it. Nothing
/// otherwise.
pub fn on_escape(s: &mut TimelineState) -> Vec<TimelineAction> {
    let before = (s.view, s.temp, s.scroll_y);
    let Some(back) = s.temp.escape() else {
        return Vec::new();
    };
    let from = s.view;
    s.view = back;
    finish(s, before, vec![TimelineAction::Animate { from }])
}

/// What a click on `hit` does.
fn click(s: &mut TimelineState, hit: Hit) -> Vec<TimelineAction> {
    let selection = match hit {
        Hit::Save(i) => Selection::Station(i),
        Hit::Now => Selection::Now,
        Hit::Group(members) => return click_group(s, members),
        // A lane's name answers a double click and the right button.
        Hit::LaneName(_) => return Vec::new(),
    };
    s.data.selection = selection;
    vec![TimelineAction::Select(selection)]
}

/// Zooms in on the group of `members` temporarily, so that it fills the middle of the width,
/// when that splits it into more than one ball. Otherwise asks for its list.
fn click_group(s: &mut TimelineState, members: Range<usize>) -> Vec<TimelineAction> {
    let Some(target) = s.group_target(members.clone()) else {
        return Vec::new();
    };
    if !splits_at(&target, &s.xs, members.clone()) {
        return vec![TimelineAction::OpenGroupList(members)];
    }
    let from = s.view;
    s.temp.begin(from);
    s.view = target;
    vec![TimelineAction::Animate { from }]
}

/// Ends `actions` with `Redraw` when the view, the temporary zoom or the scroll changed since
/// `before`, and `RequestOlder` when the view changed and needs an older page.
fn finish(
    s: &TimelineState,
    before: (View, TempZoom, f64),
    mut actions: Vec<TimelineAction>,
) -> Vec<TimelineAction> {
    let moved = s.view != before.0;
    let redraw = moved || s.temp != before.1 || s.scroll_y != before.2;
    if redraw && !actions.contains(&TimelineAction::Redraw) {
        actions.push(TimelineAction::Redraw);
    }
    if moved && needs_older_page(s) {
        actions.push(TimelineAction::RequestOlder);
    }
    actions
}

/// Moves the content `dx` screen pixels to the right, as a drag does.
fn pan(view: &mut View, dx: f64) {
    if dx.is_finite() && dx != 0.0 {
        view.pan_by(dx);
    }
}

/// Zooms by `lines` wheel lines around screen x `anchor`. At a zoom limit the view stays exactly
/// as it is.
fn zoom(view: &mut View, lines: f64, anchor: f64) {
    if !(lines.is_finite() && lines != 0.0 && anchor.is_finite()) {
        return;
    }
    let mut zoomed = *view;
    zoomed.zoom_at(WHEEL_ZOOM_STEP.powf(lines), anchor);
    if zoomed.day_px != view.day_px {
        *view = zoomed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::axis::{MAX_DAY_PX, MIN_DAY_PX};
    use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone};
    use savestates_core::SaveId;
    use savestates_ui_raven::{Event, Key, MouseButton};

    use crate::theme;

    use TimelineAction::{
        Animate, JumpToEnd, LaneMenu, OpenGroupList, Redraw, Rename, RequestOlder, Select,
    };

    const WIDTH: f64 = 1000.0;
    const HEIGHT: f64 = 240.0;
    /// The first lane's line, at scroll 0: under the 24 px header, 32 px into the lane.
    const LINE_Y: f64 = 56.0;
    /// "Agora" sits this far from the right edge on open and after jumping to the end.
    const END_MARGIN: f64 = NOW_MIN_GAP_PX + 24.0;

    /// A local time in 2026.
    fn at(month: u32, day: u32, hour: u32, minute: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, month, day)
            .and_then(|d| d.and_hms_opt(hour, minute, 0))
            .unwrap()
    }

    /// The tests' clock: 12:00 on 2 November.
    fn now() -> NaiveDateTime {
        at(11, 2, 12, 0)
    }

    /// The tests' `ToLocal`: each time's own offset.
    fn own_offset(t: DateTime<FixedOffset>) -> NaiveDateTime {
        t.naive_local()
    }

    /// "Save {n}", committed at `local` (UTC-3).
    fn save(n: usize, local: NaiveDateTime) -> SaveState {
        let committed = FixedOffset::west_opt(3 * 3600)
            .unwrap()
            .from_local_datetime(&local)
            .unwrap();
        SaveState {
            id: SaveId(format!("{n:040x}")),
            name: format!("Save {n}"),
            time: committed,
            committed,
            files_changed: 1,
        }
    }

    fn data(times: &[NaiveDateTime]) -> TimelineData {
        let saves = times.iter().enumerate().map(|(n, &t)| save(n, t)).collect();
        TimelineData::one_lane(saves, false, 3)
    }

    fn state(times: &[NaiveDateTime]) -> TimelineState {
        state_of(data(times))
    }

    fn state_of(data: TimelineData) -> TimelineState {
        TimelineState::new(data, WIDTH, HEIGHT, own_offset, now())
    }

    /// Five saves 6 hours apart (35 px at the default zoom), the newest at 06:00 today.
    fn spread() -> TimelineState {
        state(&[
            at(11, 1, 6, 0),
            at(11, 1, 12, 0),
            at(11, 1, 18, 0),
            at(11, 2, 0, 0),
            at(11, 2, 6, 0),
        ])
    }

    /// Saves at 10:00, 10:01, 10:30, 10:31 and 11:00 yesterday, one ball at the default zoom,
    /// then "you are here" at 06:00 today.
    fn grouped() -> TimelineState {
        state(&[
            at(11, 1, 10, 0),
            at(11, 1, 10, 1),
            at(11, 1, 10, 30),
            at(11, 1, 10, 31),
            at(11, 1, 11, 0),
            at(11, 2, 6, 0),
        ])
    }

    fn screen_x(s: &TimelineState, i: usize) -> f64 {
        s.view.world_to_screen(s.xs()[i])
    }

    /// The screen x of the group ball holding exactly `members`.
    fn group_x(s: &TimelineState, members: Range<usize>) -> f64 {
        let items = s.items();
        items
            .iter()
            .find_map(|item| match item {
                Item::Group { members: m, x, .. } if *m == members => Some(*x),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no group {members:?} in {items:?}"))
    }

    fn assert_close(a: f64, b: f64, tolerance: f64) {
        assert!(
            (a - b).abs() <= tolerance,
            "{a} is not within {tolerance} of {b}"
        );
    }

    fn down(x: f64, y: f64, clicks: i32) -> Event {
        Event::MouseDown {
            x: x as f32,
            y: y as f32,
            button: MouseButton::Left,
            clicks,
        }
    }

    fn up(x: f64, y: f64) -> Event {
        Event::MouseUp {
            x: x as f32,
            y: y as f32,
            button: MouseButton::Left,
        }
    }

    fn mv(x: f64, y: f64, dx: f64, dy: f64) -> Event {
        Event::MouseMove {
            x: x as f32,
            y: y as f32,
            dx: dx as f32,
            dy: dy as f32,
        }
    }

    fn wheel(x: f64, wx: f32, wy: f32, ctrl: bool, shift: bool) -> Event {
        Event::Wheel {
            x: x as f32,
            y: LINE_Y as f32,
            wx,
            wy,
            ctrl,
            shift,
        }
    }

    /// A click on the group ball holding exactly `members`.
    fn click_group(s: &mut TimelineState, members: Range<usize>) -> Vec<TimelineAction> {
        let x = group_x(s, members);
        click(s, x, LINE_Y)
    }

    /// A press and release at (`x`, `y`): what the release gives (the press gives nothing).
    fn click(s: &mut TimelineState, x: f64, y: f64) -> Vec<TimelineAction> {
        assert!(handle_event(s, &down(x, y, 1)).is_empty());
        handle_event(s, &up(x, y))
    }

    #[test]
    fn new_state_shows_agora_near_the_right_edge() {
        let s = spread();
        assert_eq!(s.axis.origin, now().date());
        assert_eq!(s.view.day_px, DEFAULT_DAY_PX);
        assert_eq!(s.view.width, WIDTH);
        assert_eq!(s.lane_line_y(0), LINE_Y);
        assert_eq!(s.now_line_y(), LINE_Y);
        assert_eq!(s.slot_count(), 1);
        assert_eq!(s.scroll_y(), 0.0);
        assert_close(s.now_screen_x(), WIDTH - END_MARGIN, 1e-9);
        assert!(!s.temp.active());
        // Saves sit at their time: 6 hours is 35 px at the default zoom.
        assert_eq!(s.xs().len(), 5);
        assert_eq!(s.xs()[4], s.axis.x_of(at(11, 2, 6, 0)));
        assert_close(screen_x(&s, 4) - screen_x(&s, 3), 35.0, 1e-9);

        // An empty project shows "Agora" alone, at the same place.
        let empty = state(&[]);
        assert!(empty.xs().is_empty() && empty.items().is_empty());
        assert_close(empty.now_screen_x(), WIDTH - END_MARGIN, 1e-9);
    }

    #[test]
    fn the_first_view_shows_you_are_here_and_agora() {
        // The last save was two weeks ago: with "Agora" near the right edge at the default zoom,
        // "Você está aqui" is far off the left edge. The first view zooms out until both fill
        // the middle 60 %.
        let mut s = state(&[at(10, 17, 9, 40), at(10, 19, 15, 45)]);
        assert!(screen_x(&s, 1) < 0.0);
        s.first_view();
        let span = s.axis.x_of(now()) - s.xs()[1];
        assert_close(s.view.day_px, 0.6 * WIDTH / span, 1e-9);
        assert_close(screen_x(&s, 1), 0.2 * WIDTH, 1e-6);
        assert_close(s.now_screen_x(), 0.8 * WIDTH, 1e-6);
        assert!(!s.temp.active());

        // When "Você está aqui" shows at the zoom it has (the default, or the one remembered),
        // the first view is the usual one: "Agora" near the right edge, at that zoom.
        let mut s = spread();
        s.view.zoom_at(2.0, 500.0);
        s.first_view();
        assert_eq!(s.view.day_px, 2.0 * DEFAULT_DAY_PX);
        assert_close(s.now_screen_x(), WIDTH - END_MARGIN, 1e-9);
        assert!((0.0..=WIDTH).contains(&screen_x(&s, 4)));

        // Years ago: as far out as the zoom goes, with "Agora" still near the right edge.
        let years_ago = NaiveDate::from_ymd_opt(2019, 3, 4)
            .and_then(|d| d.and_hms_opt(10, 0, 0))
            .unwrap();
        let mut s = state(&[years_ago]);
        s.first_view();
        assert_eq!(s.view.day_px, MIN_DAY_PX);
        assert_close(s.now_screen_x(), WIDTH - END_MARGIN, 1e-9);

        // Without saves, "Agora" near the right edge.
        let mut s = state(&[]);
        s.view.pan_by(300.0);
        s.first_view();
        assert_eq!(s.view.day_px, DEFAULT_DAY_PX);
        assert_close(s.now_screen_x(), WIDTH - END_MARGIN, 1e-9);
    }

    #[test]
    fn wheel_zooms_without_modifier() {
        let mut s = spread();
        let anchor = s.view.screen_to_world(300.0);
        let actions = handle_event(&mut s, &wheel(300.0, 0.0, 1.0, false, false));
        assert_eq!(actions, [Redraw]);
        assert_close(s.view.day_px, DEFAULT_DAY_PX * 1.15, 1e-9);
        assert_close(s.view.world_to_screen(anchor), 300.0, 1e-6);

        // Ctrl+wheel zooms the same way, and turning back zooms out.
        assert_eq!(
            handle_event(&mut s, &wheel(300.0, 0.0, 1.0, true, false)),
            [Redraw]
        );
        assert_close(s.view.day_px, DEFAULT_DAY_PX * 1.15 * 1.15, 1e-9);
        handle_event(&mut s, &wheel(300.0, 0.0, -2.0, false, false));
        assert_close(s.view.day_px, DEFAULT_DAY_PX, 1e-9);
        assert_close(s.view.world_to_screen(anchor), 300.0, 1e-6);

        // At a zoom limit nothing changes, not even by rounding, so nothing needs redrawing.
        for (day_px, wy) in [(MAX_DAY_PX, 1.0), (MIN_DAY_PX, -1.0)] {
            s.view.day_px = day_px;
            let before = s.view;
            assert!(handle_event(&mut s, &wheel(301.3, 0.0, wy, false, false)).is_empty());
            assert_eq!(s.view, before);
        }
    }

    #[test]
    fn shift_wheel_and_wx_pan() {
        let mut s = spread();
        let x2 = screen_x(&s, 2);
        let actions = handle_event(&mut s, &wheel(300.0, 0.0, 2.0, false, true));
        assert_eq!(actions, [Redraw]);
        assert_close(screen_x(&s, 2), x2 + 120.0, 1e-6);
        assert_eq!(s.view.day_px, DEFAULT_DAY_PX);

        // A horizontal wheel (a touchpad's sideways swipe) pans, with or without Shift.
        assert_eq!(
            handle_event(&mut s, &wheel(300.0, -1.0, 0.0, false, false)),
            [Redraw]
        );
        assert_close(screen_x(&s, 2), x2 + 60.0, 1e-6);
        handle_event(&mut s, &wheel(300.0, 1.0, 0.0, false, true));
        assert_close(screen_x(&s, 2), x2 + 120.0, 1e-6);
        assert_eq!(s.view.day_px, DEFAULT_DAY_PX);
    }

    #[test]
    fn drag_pans_and_click_selects() {
        let mut s = spread();
        let x1 = screen_x(&s, 1);
        // Moving without a pressed button does nothing.
        assert!(handle_event(&mut s, &mv(300.0, LINE_Y, 20.0, 0.0)).is_empty());
        assert!(handle_event(&mut s, &down(300.0, LINE_Y, 1)).is_empty());
        assert_eq!(
            handle_event(&mut s, &mv(350.0, LINE_Y, 50.0, 0.0)),
            [Redraw]
        );
        assert_close(screen_x(&s, 1), x1 + 50.0, 1e-6);
        assert_eq!(s.view.day_px, DEFAULT_DAY_PX);
        // Letting go after a pan is not a click, and the drag is over.
        assert!(handle_event(&mut s, &up(350.0, LINE_Y)).is_empty());
        assert!(handle_event(&mut s, &mv(400.0, LINE_Y, 50.0, 0.0)).is_empty());
        assert_eq!(s.data.selection, Selection::Now);

        // A click on a save selects it.
        let x1 = screen_x(&s, 1);
        assert_eq!(
            click(&mut s, x1 + 2.0, LINE_Y + 1.0),
            [Select(Selection::Station(1))]
        );
        assert_eq!(s.data.selection, Selection::Station(1));

        // A small wobble is still a click...
        let x3 = screen_x(&s, 3);
        handle_event(&mut s, &down(x3, LINE_Y, 1));
        handle_event(&mut s, &mv(x3 + 3.0, LINE_Y, 3.0, 0.0));
        assert_eq!(
            handle_event(&mut s, &up(x3 + 3.0, LINE_Y)),
            [Select(Selection::Station(3))]
        );
        // ...but moves add up, in x and y: 4 px in all is a pan.
        let x2 = screen_x(&s, 2);
        handle_event(&mut s, &down(x2, LINE_Y, 1));
        handle_event(&mut s, &mv(x2 + 2.0, LINE_Y, 2.0, 0.0));
        handle_event(&mut s, &mv(x2 + 2.0, LINE_Y + 2.0, 0.0, 2.0));
        assert!(handle_event(&mut s, &up(x2 + 2.0, LINE_Y + 2.0)).is_empty());
        assert_eq!(s.data.selection, Selection::Station(3));

        // A click on "Agora" selects Now; one on empty space selects nothing.
        let now_x = s.now_screen_x();
        assert_eq!(click(&mut s, now_x, LINE_Y), [Select(Selection::Now)]);
        assert_eq!(s.data.selection, Selection::Now);
        let gap = (screen_x(&s, 1) + screen_x(&s, 2)) / 2.0;
        assert!(click(&mut s, gap, LINE_Y).is_empty());

        // Another button selects nothing.
        let x2 = screen_x(&s, 2);
        let right_down = Event::MouseDown {
            x: x2 as f32,
            y: LINE_Y as f32,
            button: MouseButton::Right,
            clicks: 1,
        };
        let right_up = Event::MouseUp {
            x: x2 as f32,
            y: LINE_Y as f32,
            button: MouseButton::Right,
        };
        assert!(handle_event(&mut s, &right_down).is_empty());
        assert!(handle_event(&mut s, &right_up).is_empty());
        assert_eq!(s.data.selection, Selection::Now);
    }

    #[test]
    fn group_click_temporary_zoom_and_escape() {
        let mut s = grouped();
        let before = s.view;
        let actions = click_group(&mut s, 0..5);
        assert_eq!(actions, [Animate { from: before }, Redraw]);
        assert!(s.temp.active());
        // The group fills the middle 60 %.
        assert_close(screen_x(&s, 0), 0.2 * WIDTH, 1e-6);
        assert_close(screen_x(&s, 4), 0.8 * WIDTH, 1e-6);
        assert!(s.items().len() > 2);

        let zoomed = s.view;
        assert_eq!(on_escape(&mut s), [Animate { from: zoomed }, Redraw]);
        assert_eq!(s.view, before);
        assert!(!s.temp.active());
        assert!(on_escape(&mut s).is_empty());
        assert_eq!(s.view, before);
    }

    #[test]
    fn nested_group_click_keeps_original_return() {
        let mut s = grouped();
        let before = s.view;
        click_group(&mut s, 0..5);
        // Zoomed in, 10:00 and 10:01 still share a ball.
        let first_zoom = s.view;
        assert_eq!(
            click_group(&mut s, 0..2),
            [Animate { from: first_zoom }, Redraw]
        );
        assert!(s.view.day_px > first_zoom.day_px);
        assert!(s.temp.active());

        let second_zoom = s.view;
        assert_eq!(on_escape(&mut s), [Animate { from: second_zoom }, Redraw]);
        assert_eq!(s.view, before);
        assert!(!s.temp.active());
    }

    #[test]
    fn pan_or_zoom_accepts() {
        let wheel_zoom = vec![wheel(300.0, 0.0, 1.0, false, false)];
        let shift_pan = vec![wheel(300.0, 0.0, 1.0, false, true)];
        let sideways = vec![wheel(300.0, 1.0, 0.0, false, false)];
        let drag = vec![
            down(300.0, LINE_Y, 1),
            mv(340.0, LINE_Y, 40.0, 0.0),
            up(340.0, LINE_Y),
        ];
        for events in [wheel_zoom, shift_pan, sideways, drag] {
            let mut s = grouped();
            click_group(&mut s, 0..5);
            assert!(s.temp.active());
            for ev in &events {
                handle_event(&mut s, ev);
            }
            assert!(!s.temp.active(), "{events:?}");
            let view = s.view;
            assert!(on_escape(&mut s).is_empty());
            assert_eq!(s.view, view);
        }

        // Even at a zoom limit, where the view can't change, the wheel ends it (and the
        // pill has to go).
        let mut s = grouped();
        click_group(&mut s, 0..5);
        s.view.day_px = MAX_DAY_PX;
        assert_eq!(
            handle_event(&mut s, &wheel(300.0, 0.0, 1.0, false, false)),
            [Redraw]
        );
        assert!(!s.temp.active());
    }

    #[test]
    fn peek_keeps_temp_zoom() {
        let mut s = grouped();
        let before = s.view;
        click_group(&mut s, 0..5);
        // 11:00, the group's last save, has its own ball now; peek at it with a small wobble.
        let x4 = screen_x(&s, 4);
        handle_event(&mut s, &down(x4, LINE_Y, 1));
        handle_event(&mut s, &mv(x4 + 2.0, LINE_Y, 2.0, 0.0));
        assert_eq!(
            handle_event(&mut s, &up(x4 + 2.0, LINE_Y)),
            [Select(Selection::Station(4))]
        );
        assert!(s.temp.active());
        // Esc still goes back to where the user started.
        assert_eq!(on_escape(&mut s).len(), 2);
        assert_eq!(s.view, before);
        assert_eq!(s.data.selection, Selection::Station(4));
    }

    #[test]
    fn jump_to_end_accepts() {
        // Three saves a minute apart, then "you are here" at 11:55; "Agora" is at 12:00.
        let near_now = || {
            state(&[
                at(11, 2, 11, 50),
                at(11, 2, 11, 51),
                at(11, 2, 11, 52),
                at(11, 2, 11, 55),
            ])
        };
        let mut s = near_now();
        click_group(&mut s, 0..3);
        assert!(s.temp.active());
        let day_px = s.view.day_px;
        s.jump_to_end();
        assert!(!s.temp.active());
        assert_eq!(s.view.day_px, day_px);
        assert_close(s.now_screen_x(), WIDTH - END_MARGIN, 1e-6);
        assert!(on_escape(&mut s).is_empty());

        // A double click on "Agora", in view during the temporary zoom, does the same.
        let mut s = near_now();
        click_group(&mut s, 0..3);
        let now_x = s.now_screen_x();
        assert!((0.0..WIDTH).contains(&now_x), "{now_x}");
        // Its first click selects Now, which keeps the temporary zoom.
        assert_eq!(click(&mut s, now_x, LINE_Y), [Select(Selection::Now)]);
        assert!(s.temp.active());
        assert_eq!(
            handle_event(&mut s, &down(now_x, LINE_Y, 2)),
            [JumpToEnd, Redraw]
        );
        assert!(!s.temp.active());
        assert_close(s.now_screen_x(), WIDTH - END_MARGIN, 1e-6);
        // Letting go afterwards is not a click.
        assert!(handle_event(&mut s, &up(now_x, LINE_Y)).is_empty());

        // A double click elsewhere does nothing.
        let before = s.view;
        let x0 = screen_x(&s, 0);
        assert!(handle_event(&mut s, &down(x0, LINE_Y, 2)).is_empty());
        assert!(handle_event(&mut s, &mv(x0 + 50.0, LINE_Y, 50.0, 0.0)).is_empty());
        assert!(handle_event(&mut s, &up(x0 + 50.0, LINE_Y)).is_empty());
        assert_eq!(s.view, before);
    }

    #[test]
    fn unsplittable_group_opens_list() {
        // Six saves made in the same second, then "you are here" an hour later.
        let mut times = vec![at(11, 2, 10, 0); 6];
        times.push(at(11, 2, 11, 0));
        for day_px in [MIN_DAY_PX, DEFAULT_DAY_PX, 5_000.0, MAX_DAY_PX] {
            let mut s = state(&times);
            s.view = View {
                day_px,
                left: s.xs()[0] - WIDTH / 2.0 / day_px,
                width: WIDTH,
            };
            let before = s.view;
            assert_eq!(
                click_group(&mut s, 0..6),
                [OpenGroupList(0..6)],
                "at {day_px} px a day"
            );
            assert_eq!(s.view, before);
            assert!(!s.temp.active());
        }
    }

    #[test]
    fn now_gap_after_save() {
        let mut s = state(&[at(11, 2, 9, 0), at(11, 2, 11, 59)]);
        for day_px in [MIN_DAY_PX, DEFAULT_DAY_PX, 2_000.0, MAX_DAY_PX] {
            // Zoomed around "you are here", so that it stays in view.
            let factor = day_px / s.view.day_px;
            s.view.zoom_at(factor, screen_x(&s, 1));
            let head = screen_x(&s, 1);
            let now_x = s.now_screen_x();
            assert!(now_x >= head + NOW_MIN_GAP_PX, "{now_x} vs {head}");
            // Each is clicked on its own.
            let hit = |x: f64| s.hit_at(x, LINE_Y);
            assert_eq!(hit(head), Some(Hit::Save(1)));
            assert_eq!(hit(now_x), Some(Hit::Now));
        }
    }

    #[test]
    fn older_page_by_time() {
        // The oldest loaded save is a month older than the others: 4,445 px left of "you are
        // here" at the default zoom, so 3,581 px left of the widget's left edge.
        let times = [at(10, 1, 12, 0), at(11, 1, 6, 0), at(11, 2, 6, 0)];
        let with_more = || {
            let mut s = state(&times);
            s.data.more_available = true;
            s
        };
        assert!(!needs_older_page(&with_more()));
        let drag_to_the_past = |more_available: bool, dx: f64| {
            let mut s = state(&times);
            s.data.more_available = more_available;
            handle_event(&mut s, &down(300.0, LINE_Y, 1));
            let actions = handle_event(&mut s, &mv(300.0 + dx, LINE_Y, dx, 0.0));
            (actions, s)
        };
        // Less than a screen width from it: load the page before it.
        let (near, s) = drag_to_the_past(true, 2_700.0);
        assert_eq!(near, [Redraw, RequestOlder]);
        assert!(needs_older_page(&s));
        // With nothing more to load, only a redraw.
        assert_eq!(drag_to_the_past(false, 2_700.0).0, [Redraw]);
        // Further than a screen width: nothing to load yet.
        let (far, s) = drag_to_the_past(true, 2_500.0);
        assert_eq!(far, [Redraw]);
        assert!(!needs_older_page(&s));

        // Zooming out brings it near too.
        let mut s = with_more();
        assert_eq!(
            handle_event(&mut s, &wheel(900.0, 0.0, -8.0, false, false)),
            [Redraw, RequestOlder]
        );
    }

    #[test]
    fn older_saves_in_new_data_keep_the_view() {
        let mut s = spread();
        s.data.more_available = true;
        let view = s.view;
        let x1 = screen_x(&s, 1);
        // Two older saves come in front, through new data: world x comes from time, so the
        // loaded saves stay where they were.
        let mut saves = vec![save(100, at(10, 31, 6, 0)), save(101, at(10, 31, 18, 0))];
        saves.extend(s.data.saves.iter().cloned());
        let mut older = TimelineData::one_lane(saves, false, 3);
        older.selection = Selection::Station(3);
        s.replace_data(older, now());
        assert_eq!(s.view, view);
        assert_eq!(s.data.selection, Selection::Station(3));
        assert_eq!(screen_x(&s, 3), x1);
        assert!(!s.data.more_available);
        assert_eq!(s.xs().len(), 7);
        assert_eq!(s.xs()[0], s.axis.x_of(at(10, 31, 6, 0)));
        assert_eq!(s.data.you_are_here(), Some(6));

        // History order holds across the old boundary too: a save dated after the ones after
        // it pushes them right, never left.
        let mut saves = vec![save(102, at(11, 1, 23, 0))];
        saves.extend(s.data.saves.iter().cloned());
        s.replace_data(TimelineData::one_lane(saves, true, 3), now());
        assert!(s.xs().windows(2).all(|pair| pair[0] < pair[1]));
        assert!(s.data.more_available);
    }

    #[test]
    fn set_size_keeps_the_right_edge() {
        let mut s = grouped();
        let before = s.view;
        click_group(&mut s, 0..5);
        s.set_size(600.0, HEIGHT);
        assert_eq!(s.view.width, 600.0);
        // Esc goes back to the view from before, at the new width, with "Agora" where it was
        // from the right edge.
        on_escape(&mut s);
        assert_eq!(s.view.width, 600.0);
        assert_eq!(s.view.day_px, before.day_px);
        assert_close(s.now_screen_x(), 600.0 - END_MARGIN, 1e-6);
    }

    #[test]
    fn refresh_keeps_the_view_and_the_clock_moves_agora() {
        let mut s = spread();
        let x2 = screen_x(&s, 2);
        click(&mut s, x2, LINE_Y);
        let view = s.view;
        // At 12:00, 35 px after "you are here", "Agora" keeps the minimum gap. By 18:00 it is
        // 70 px after it, at its own time. Nothing else moves.
        assert_close(s.now_screen_x(), screen_x(&s, 4) + NOW_MIN_GAP_PX, 1e-9);
        s.set_now(at(11, 2, 18, 0));
        assert_close(s.now_screen_x(), screen_x(&s, 4) + 70.0, 1e-6);
        assert_eq!(s.view, view);
        // New data keeps the view and places the saves again.
        let mut fresh = data(&[at(11, 1, 6, 0), at(11, 2, 12, 30)]);
        fresh.selection = Selection::Station(1);
        s.replace_data(fresh, at(11, 2, 18, 0));
        assert_eq!(s.view, view);
        assert_eq!(s.xs().len(), 2);
        assert_eq!(s.xs()[1], s.axis.x_of(at(11, 2, 12, 30)));
        // A save dated after now is drawn at now.
        s.set_now(at(11, 2, 12, 10));
        assert_eq!(s.xs()[1], s.axis.x_of(at(11, 2, 12, 10)));
    }

    #[test]
    fn a_new_save_in_new_data_keeps_the_view_and_the_clock_moves() {
        let mut s = spread();
        s.data.selection = Selection::Station(2);
        let view = s.view;
        // A save made at 12:30, now that it is 12:31.
        let mut saves = s.data.saves.clone();
        saves.push(save(5, at(11, 2, 12, 30)));
        let mut fresh = TimelineData::one_lane(saves, false, 0);
        fresh.selection = s.data.selection;
        s.replace_data(fresh, at(11, 2, 12, 31));
        assert_eq!(s.data.saves.len(), 6);
        assert_eq!(s.data.you_are_here(), Some(5));
        assert_eq!(s.xs()[5], s.axis.x_of(at(11, 2, 12, 30)));
        assert_eq!(s.view, view);
        assert_eq!(s.data.selection, Selection::Station(2));
        // "Agora" is a minute later, so it keeps its minimum gap from the new save.
        assert_close(s.now_screen_x(), screen_x(&s, 5) + NOW_MIN_GAP_PX, 1e-9);
    }

    #[test]
    fn group_splits_like_a_click_on_it() {
        let s = grouped();
        assert!(s.group_splits(0..5));
        let mut times = vec![at(11, 2, 10, 0); 6];
        times.push(at(11, 2, 11, 0));
        assert!(!state(&times).group_splits(0..6));
        assert!(!s.group_splits(0..0));
    }

    #[test]
    fn temp_zoom_keeps_the_first_return_point() {
        let a = View {
            day_px: 100.0,
            left: 1.0,
            width: 800.0,
        };
        let b = View { day_px: 900.0, ..a };
        let mut temp = TempZoom::default();
        assert!(!temp.active());
        assert_eq!(temp.escape(), None);
        temp.begin(a);
        temp.begin(b);
        assert!(temp.active());
        assert_eq!(temp.escape(), Some(a));
        assert!(!temp.active());
        temp.begin(b);
        temp.accept();
        assert!(!temp.active());
        assert_eq!(temp.escape(), None);
    }

    #[test]
    fn leave_cancels_a_drag() {
        let mut s = spread();
        handle_event(&mut s, &down(300.0, LINE_Y, 1));
        handle_event(&mut s, &mv(310.0, LINE_Y, 10.0, 0.0));
        assert!(handle_event(&mut s, &Event::Leave).is_empty());
        let view = s.view;
        assert!(handle_event(&mut s, &mv(400.0, LINE_Y, 90.0, 0.0)).is_empty());
        assert_eq!(s.view, view);
        assert!(handle_event(&mut s, &up(400.0, LINE_Y)).is_empty());
    }

    #[test]
    fn other_events_are_ignored() {
        let mut s = spread();
        let before = s.view;
        assert!(handle_event(&mut s, &Event::Enter).is_empty());
        let key = Event::KeyDown {
            key: Key::Escape,
            ctrl: false,
            shift: false,
        };
        assert!(handle_event(&mut s, &key).is_empty());
        assert_eq!(s.view, before);
    }

    /// One lane per entry of `times`, with saves at those local times, numbered lane by lane.
    /// Lane 0 is the current timeline, and "Você está aqui" is its newest save.
    fn lanes(times: &[&[NaiveDateTime]]) -> TimelineData {
        let mut saves = Vec::new();
        let mut lanes = Vec::new();
        for (k, lane) in times.iter().enumerate() {
            let start = saves.len();
            for &t in *lane {
                saves.push(save(saves.len(), t));
            }
            lanes.push(LaneData {
                label: format!("Linha {k}"),
                branch: Some(format!("linha-{k}")),
                color: theme::LINE,
                saves: start..saves.len(),
                fork: None,
                continues_left: false,
                archived: false,
            });
        }
        let here = lanes
            .first()
            .and_then(|l: &LaneData| l.saves.end.checked_sub(1));
        TimelineData {
            saves,
            lanes,
            tags: Vec::new(),
            here,
            current_lane: Some(0),
            pending_after: None,
            more_available: false,
            unsaved: 3,
            selection: Selection::Now,
        }
    }

    /// `count` lanes of one save each, at 06:00 today.
    fn many_lanes(count: usize) -> TimelineData {
        let one = [at(11, 2, 6, 0)];
        lanes(&vec![&one[..]; count])
    }

    fn right(x: f64, y: f64, down: bool) -> Event {
        let (x, y) = (x as f32, y as f32);
        if down {
            Event::MouseDown {
                x,
                y,
                button: MouseButton::Right,
                clicks: 1,
            }
        } else {
            Event::MouseUp {
                x,
                y,
                button: MouseButton::Right,
            }
        }
    }

    #[test]
    fn lanes_stack_under_the_header() {
        let mut s = TimelineState::new(many_lanes(3), WIDTH, 400.0, own_offset, now());
        assert_eq!(HEADER_PX, 24.0);
        assert_eq!(LANE_PX, 76.0);
        assert_eq!(LINE_OFFSET_PX, 32.0);
        let ys: Vec<f64> = (0..3).map(|lane| s.lane_line_y(lane)).collect();
        assert_eq!(ys, [56.0, 132.0, 208.0]);
        assert_eq!(s.slot_count(), 3);
        // "Agora" is on the current lane.
        assert_eq!(s.now_line_y(), 56.0);
        s.data.current_lane = Some(2);
        assert_eq!(s.now_line_y(), 208.0);
        // Every save knows its lane.
        let lanes: Vec<Option<usize>> = (0..4).map(|i| s.lane_of(i)).collect();
        assert_eq!(lanes, [Some(0), Some(1), Some(2), None]);
        // Without a current lane or a pending one, "Agora" is on the first slot.
        s.data.current_lane = None;
        assert_eq!(s.now_line_y(), 56.0);
    }

    #[test]
    fn the_pending_slot_sits_after_its_parent() {
        let mut data = many_lanes(3);
        data.current_lane = None;
        data.pending_after = Some(0);
        let s = TimelineState::new(data, WIDTH, 400.0, own_offset, now());
        assert_eq!(s.slot_count(), 4);
        assert_eq!(s.lane_line_y(0), 56.0);
        assert_eq!(s.now_line_y(), 132.0);
        assert_eq!(s.lane_line_y(1), 208.0);
        assert_eq!(s.lane_line_y(2), 284.0);
    }

    #[test]
    fn a_branch_starts_after_its_fork() {
        // The main line at 10:00 and 12:00 yesterday; a timeline started from 12:00 whose first
        // save says 11:00 (a clock set wrong, or a rebase), then one at 13:00.
        let mut data = lanes(&[
            &[at(11, 1, 10, 0), at(11, 1, 12, 0)],
            &[at(11, 1, 11, 0), at(11, 1, 13, 0)],
        ]);
        data.lanes[1].fork = Some(1);
        let s = state_of(data);
        let xs = s.xs();
        // Just right of the fork: one second after it.
        assert!(xs[2] > xs[1], "{xs:?}");
        assert_close(xs[2] - xs[1], 1.0 / 86_400.0, 1e-9);
        // The rest of the lane keeps its own time.
        assert_eq!(xs[3], s.axis.x_of(at(11, 1, 13, 0)));
        // A lane dated after its fork keeps its time.
        let mut data = lanes(&[&[at(11, 1, 10, 0)], &[at(11, 1, 11, 0)]]);
        data.lanes[1].fork = Some(0);
        let s = state_of(data);
        assert_eq!(s.xs()[1], s.axis.x_of(at(11, 1, 11, 0)));
    }

    #[test]
    fn hits_stay_in_their_own_lane() {
        let t = at(11, 2, 6, 0);
        let mut s = state_of(lanes(&[&[t], &[t]]));
        let x = screen_x(&s, 1);
        assert_eq!(screen_x(&s, 0), x);
        let lane1 = s.lane_line_y(1);
        assert_eq!(s.hit_at(x, lane1), Some(Hit::Save(1)));
        assert_eq!(click(&mut s, x, lane1), [Select(Selection::Station(1))]);
        assert_eq!(click(&mut s, x, LINE_Y), [Select(Selection::Station(0))]);
        // "Agora" is only hit in its own slot.
        let now_x = s.now_screen_x();
        assert_eq!(s.hit_at(now_x, LINE_Y), Some(Hit::Now));
        assert_eq!(s.hit_at(now_x, lane1), None);
        // The header and below the last slot hit nothing.
        assert_eq!(s.hit_at(x, 10.0), None);
        assert_eq!(s.hit_at(x, HEIGHT - 2.0), None);
    }

    #[test]
    fn groups_never_cross_lanes() {
        // A minute apart: one ball on a single line, but one per lane here.
        let mut data = lanes(&[&[at(11, 1, 10, 0)], &[at(11, 1, 10, 1)]]);
        data.here = None;
        let s = state_of(data);
        let items = s.items();
        assert_eq!(items.len(), 2, "{items:?}");
        assert!(matches!(items[0], Item::Single { index: 0, .. }));
        assert!(matches!(items[1], Item::Single { index: 1, .. }));
        // Within a lane they still group.
        let mut data = lanes(&[&[at(11, 1, 10, 0), at(11, 1, 10, 1)], &[at(11, 1, 10, 2)]]);
        data.here = None;
        let items = state_of(data).items();
        assert_eq!(items.len(), 2, "{items:?}");
        assert!(matches!(items[0], Item::Group { ref members, .. } if *members == (0..2)));
    }

    #[test]
    fn right_click_on_a_lane_name_opens_its_menu() {
        let mut s = state_of(many_lanes(2));
        s.set_label_widths(vec![50.0, 80.0]);
        // Lane 1's top is at 100 px; its name box is 104 px wide and 18 px tall.
        assert_eq!(s.hit_at(10.0, 105.0), Some(Hit::LaneName(1)));
        assert_eq!(s.hit_at(103.0, 117.0), Some(Hit::LaneName(1)));
        assert_eq!(s.hit_at(105.0, 105.0), None);
        assert_eq!(s.hit_at(10.0, 119.0), None);
        assert_eq!(s.hit_at(70.0, 30.0), Some(Hit::LaneName(0)));
        assert_eq!(s.hit_at(75.0, 30.0), None);
        assert!(handle_event(&mut s, &right(10.0, 105.0, true)).is_empty());
        assert_eq!(
            handle_event(&mut s, &right(10.0, 105.0, false)),
            [LaneMenu {
                lane: 1,
                x: 10.0,
                y: 105.0
            }]
        );
        // Elsewhere, the right button does nothing.
        assert!(handle_event(&mut s, &right(500.0, 105.0, false)).is_empty());
        // A left click on a name selects nothing.
        assert!(click(&mut s, 10.0, 105.0).is_empty());
        assert_eq!(s.data.selection, Selection::Now);
    }

    #[test]
    fn double_click_on_a_lane_name_renames() {
        let mut s = state_of(many_lanes(2));
        s.set_label_widths(vec![50.0, 80.0]);
        assert_eq!(handle_event(&mut s, &down(10.0, 105.0, 2)), [Rename(1)]);
        assert!(handle_event(&mut s, &up(10.0, 105.0)).is_empty());
        assert_eq!(handle_event(&mut s, &down(10.0, 30.0, 2)), [Rename(0)]);
        // Not on a name: nothing.
        assert!(handle_event(&mut s, &down(400.0, 105.0, 2)).is_empty());
    }

    #[test]
    fn vertical_drag_scrolls_within_bounds() {
        // Six lanes are 24 + 6 × 76 = 480 px tall: 280 px more than the view.
        let mut s = TimelineState::new(many_lanes(6), WIDTH, 200.0, own_offset, now());
        assert_eq!(s.scroll_y(), 0.0);
        let view = s.view;
        handle_event(&mut s, &down(300.0, 100.0, 1));
        assert_eq!(handle_event(&mut s, &mv(300.0, 90.0, 0.0, -10.0)), [Redraw]);
        assert_eq!(s.scroll_y(), 10.0);
        assert_eq!(s.lane_line_y(0), 46.0);
        handle_event(&mut s, &mv(300.0, -910.0, 0.0, -1000.0));
        assert_eq!(s.scroll_y(), 280.0);
        assert_eq!(s.view, view);
        // At the end, more of the same changes nothing.
        assert!(handle_event(&mut s, &mv(300.0, -920.0, 0.0, -10.0)).is_empty());
        handle_event(&mut s, &mv(300.0, 4000.0, 0.0, 5000.0));
        assert_eq!(s.scroll_y(), 0.0);
        // A diagonal drag pans both ways.
        handle_event(&mut s, &mv(330.0, 3980.0, 30.0, -20.0));
        assert_eq!(s.scroll_y(), 20.0);
        assert_close(s.view.world_to_screen(view.left), 30.0, 1e-9);
        assert!(handle_event(&mut s, &up(330.0, 3980.0)).is_empty());

        // The wheel still zooms, and leaves the scroll alone.
        handle_event(&mut s, &wheel(300.0, 0.0, 1.0, false, false));
        assert_eq!(s.scroll_y(), 20.0);
        assert_close(s.view.day_px, DEFAULT_DAY_PX * 1.15, 1e-9);

        // A taller view clamps the scroll; new data keeps it, clamped.
        handle_event(&mut s, &down(300.0, 100.0, 1));
        handle_event(&mut s, &mv(300.0, -900.0, 0.0, -1000.0));
        handle_event(&mut s, &up(300.0, -900.0));
        s.set_size(WIDTH, 400.0);
        assert_eq!(s.scroll_y(), 80.0);
        s.replace_data(many_lanes(5), now());
        assert_eq!(s.scroll_y(), 4.0);
        // One lane fits: no scrolling.
        s.replace_data(many_lanes(1), now());
        assert_eq!(s.scroll_y(), 0.0);
        handle_event(&mut s, &down(300.0, 100.0, 1));
        assert!(handle_event(&mut s, &mv(300.0, 50.0, 0.0, -50.0)).is_empty());
        assert_eq!(s.scroll_y(), 0.0);
    }

    #[test]
    fn the_scroll_bar_drags_in_proportion() {
        // 480 px of content in a 200 px view: the lanes move 480 / 200 as far as the thumb.
        let mut s = TimelineState::new(many_lanes(6), WIDTH, 200.0, own_offset, now());
        let view = s.view;
        assert!(handle_event(&mut s, &down(WIDTH - 5.0, 20.0, 1)).is_empty());
        assert_eq!(
            handle_event(&mut s, &mv(WIDTH - 2.0, 30.0, 3.0, 10.0)),
            [Redraw]
        );
        assert_close(s.scroll_y(), 24.0, 1e-9);
        // It does not pan, and letting go is not a click.
        assert_eq!(s.view, view);
        assert!(handle_event(&mut s, &up(WIDTH - 2.0, 30.0)).is_empty());
        handle_event(&mut s, &mv(WIDTH - 2.0, 60.0, 0.0, 30.0));
        assert_close(s.scroll_y(), 24.0, 1e-9);
        // When everything fits there is no bar: a press there pans, as anywhere.
        let mut s = TimelineState::new(many_lanes(2), WIDTH, 200.0, own_offset, now());
        handle_event(&mut s, &down(WIDTH - 5.0, 20.0, 1));
        handle_event(&mut s, &mv(WIDTH + 15.0, 20.0, 20.0, 0.0));
        assert_close(s.view.world_to_screen(view.left), 20.0, 1e-9);
    }

    #[test]
    fn jump_to_end_reveals_agoras_slot() {
        let mut data = many_lanes(6);
        data.current_lane = Some(5);
        let mut s = TimelineState::new(data, WIDTH, 200.0, own_offset, now());
        assert!(s.now_line_y() > 200.0);
        s.view.pan_by(-300.0);
        s.jump_to_end();
        assert_eq!(s.scroll_y(), 280.0);
        assert!((HEADER_PX..200.0).contains(&s.now_line_y()));
        assert_close(s.now_screen_x(), WIDTH - END_MARGIN, 1e-9);
        // Back up at the first lane, it scrolls up to it.
        s.data.current_lane = Some(0);
        s.jump_to_end();
        assert_eq!(s.scroll_y(), 0.0);
        // A slot already in view stays where it is.
        s.data.current_lane = Some(1);
        s.jump_to_end();
        assert_eq!(s.scroll_y(), 0.0);
        // The first view too: the pending slot after lane 4 is slot 5.
        let mut data = many_lanes(6);
        data.current_lane = None;
        data.pending_after = Some(4);
        let mut s = TimelineState::new(data, WIDTH, 200.0, own_offset, now());
        s.first_view();
        assert!((HEADER_PX..200.0).contains(&s.now_line_y()));
        assert_eq!(s.scroll_y(), 24.0 + 6.0 * 76.0 - 200.0);
    }

    #[test]
    fn here_need_not_be_the_newest_save() {
        // Loaded into the middle of the line: "Você está aqui" is 10:00, the line goes on to
        // 11:59, and the next save starts a new timeline in the pending slot.
        let mut data = lanes(&[&[at(11, 2, 9, 0), at(11, 2, 10, 0), at(11, 2, 11, 59)]]);
        data.here = Some(1);
        data.current_lane = None;
        data.pending_after = Some(0);
        let s = state_of(data);
        assert_eq!(s.data.you_are_here(), Some(1));
        // 12:00 is only 2 hours (12 px) after 10:00: "Agora" keeps its 56 px gap from it.
        assert_close(s.now_screen_x(), screen_x(&s, 1) + NOW_MIN_GAP_PX, 1e-9);
        assert_eq!(s.now_line_y(), 132.0);
        let now_x = s.now_screen_x();
        assert_eq!(s.hit_at(now_x, 132.0), Some(Hit::Now));
        assert_ne!(s.hit_at(now_x, LINE_Y), Some(Hit::Now));
        // "Você está aqui" is pinned: never grouped, even mid-lane.
        assert!(
            s.items()
                .iter()
                .any(|item| matches!(*item, Item::Single { index: 1, .. }))
        );
    }

    #[test]
    fn older_pages_are_judged_across_lanes() {
        // Lane 0's oldest save is yesterday, in view; lane 1 goes back a month, 3,581 px left
        // of the widget's left edge.
        let mut data = lanes(&[&[at(11, 1, 6, 0), at(11, 2, 6, 0)], &[at(10, 1, 12, 0)]]);
        data.more_available = true;
        let s = state_of(data.clone());
        assert!(!needs_older_page(&s));
        let mut s = state_of(data);
        handle_event(&mut s, &down(300.0, LINE_Y, 1));
        let actions = handle_event(&mut s, &mv(3000.0, LINE_Y, 2_700.0, 0.0));
        assert_eq!(actions, [Redraw, RequestOlder]);
    }

    #[test]
    fn ten_thousand_saves_in_eight_lanes_lay_out_quickly() {
        let mut times: Vec<Vec<NaiveDateTime>> = Vec::new();
        for lane in 0..8 {
            let start = at(10, 1, 0, 0) + chrono::TimeDelta::minutes(lane);
            times.push(
                (0..1_250)
                    .map(|i| start + chrono::TimeDelta::minutes(8 * i))
                    .collect(),
            );
        }
        let refs: Vec<&[NaiveDateTime]> = times.iter().map(Vec::as_slice).collect();
        let mut data = lanes(&refs);
        data.selection = Selection::Station(5_000);
        let mut s = TimelineState::new(data, WIDTH, 300.0, own_offset, now());
        s.view = View {
            day_px: MIN_DAY_PX,
            left: s.axis.x_of(at(9, 30, 0, 0)),
            width: WIDTH,
        };
        let mut best = std::time::Duration::MAX;
        let mut items = Vec::new();
        for _ in 0..3 {
            let start = std::time::Instant::now();
            items = s.items();
            best = best.min(start.elapsed());
        }
        assert!(best < std::time::Duration::from_millis(20), "took {best:?}");
        // Every save is drawn once, and no ball holds saves of two lanes.
        let drawn: usize = items
            .iter()
            .map(|item| match item {
                Item::Single { .. } => 1,
                Item::Group { members, .. } => members.len(),
            })
            .sum();
        assert_eq!(drawn, 10_000);
        for item in &items {
            if let Item::Group { members, .. } = item {
                assert_eq!(s.lane_of(members.start), s.lane_of(members.end - 1));
            }
        }
    }

    #[test]
    fn truncate() {
        assert_eq!(
            truncate_label("Corrige o menu no celular", 10),
            "Corrige o…"
        );
        // Short labels, and labels that exactly fit, are unchanged.
        assert_eq!(truncate_label("Oi", 10), "Oi");
        assert_eq!(truncate_label("0123456789", 10), "0123456789");
        // Counts chars, not bytes, and never splits one.
        assert_eq!(truncate_label("ááááááá", 5), "áááá…");
        assert_eq!(truncate_label("ação é útil", 11), "ação é útil");
        // A cut that would leave a trailing space drops it.
        assert_eq!(truncate_label("Correção do botão", 10), "Correção…");
        assert_eq!(truncate_label("abc", 1), "…");
        assert_eq!(truncate_label("abc", 0), "");
        assert_eq!(truncate_label("", 0), "");
    }
}
