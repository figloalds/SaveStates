//! Pure model of the timeline: saves placed by time on a day grid, a pannable and zoomable view,
//! grouping, hit testing, the temporary zoom, paging and input handling. Nothing here draws,
//! calls Raven or reads the clock; the widget paints this state and feeds it events.
//!
//! The line holds the saves, oldest on the left, each at its local commit time (see
//! [`super::axis`]), then "Agora" for the unsaved changes at the current time, at least
//! [`NOW_MIN_GAP_PX`] right of "Você está aqui". Saves whose balls would overlap share a group
//! ball (see [`super::layout`]). Clicking a group zooms in on it temporarily: Esc goes back to
//! the view from before, and any other pan or zoom keeps the new view.

use std::ops::Range;

use chrono::{DateTime, FixedOffset, NaiveDateTime};
use savestates_core::SaveState;
use savestates_ui_raven::{Event, MouseButton};

use super::axis::{self, Axis, DEFAULT_DAY_PX, Days, NOW_MIN_GAP_PX, ToLocal, View, place_saves};
use super::layout::{Hit, Item, hit_test, layout, layout_range, splits_at};

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

/// What is selected on the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    /// "Agora": the unsaved changes.
    Now,
    /// The save at this index in [`TimelineData::saves`].
    Station(usize),
}

#[derive(Clone, Debug)]
pub struct TimelineData {
    /// Oldest first.
    pub saves: Vec<SaveState>,
    /// Whether older saves exist beyond the ones loaded.
    pub more_available: bool,
    /// How many files have unsaved changes.
    pub unsaved: usize,
    pub selection: Selection,
    /// The project has other timelines (branches) that this view does not show.
    pub other_timelines: bool,
}

impl TimelineData {
    /// The index of the newest save, the one the project is at. `None` without saves.
    pub fn you_are_here(&self) -> Option<usize> {
        self.saves.len().checked_sub(1)
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

#[derive(Clone, Debug)]
pub struct TimelineState {
    pub data: TimelineData,
    pub view: View,
    /// The y of the line, in widget coordinates.
    pub line_y: f64,
    pub axis: Axis,
    pub temp: TempZoom,
    /// The world x of each save in `data.saves`.
    xs: Vec<Days>,
    /// The world x of the current time.
    now_x: Days,
    /// While the left button is held: how far the pointer has moved since it went down.
    drag: Option<f64>,
    to_local: ToLocal,
    /// The current local time, which placing the saves needs: a save dated later sits at it.
    now_local: NaiveDateTime,
    /// The oldest save whose date could be read. Paging is judged by it.
    oldest_dated: Option<usize>,
}

impl TimelineState {
    /// A state showing `data` in a widget `width` px wide, at the default zoom, with "Agora"
    /// near the right edge. The axis starts on the day of `now_local`.
    pub fn new(
        data: TimelineData,
        width: f64,
        line_y: f64,
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
            line_y,
            axis,
            temp: TempZoom::default(),
            xs: Vec::new(),
            now_x: axis.x_of(now_local),
            drag: None,
            to_local,
            now_local,
            oldest_dated: None,
        };
        state.place_saves();
        state.put_now_at_end();
        state
    }

    /// Shows `data` instead, keeping the view, with "Agora" moved to `now_local`.
    pub fn replace_data(&mut self, data: TimelineData, now_local: NaiveDateTime) {
        self.data = data;
        self.set_now(now_local);
    }

    /// Adds `save`, just made, after the newest save, with "Agora" moved to `now_local`. The view
    /// and the selection stay.
    pub fn append_save(&mut self, save: SaveState, now_local: NaiveDateTime) {
        self.data.saves.push(save);
        self.set_now(now_local);
    }

    /// Adds a page of older saves, given newest first, before the loaded ones. The view stays:
    /// world x comes from time, so the loaded saves stay where they were. The selection moves
    /// with its save. `more_available` tells whether saves older than this page exist.
    pub fn prepend_older(&mut self, page_newest_first: Vec<SaveState>, more_available: bool) {
        self.data.more_available = more_available;
        let added = page_newest_first.len();
        if added == 0 {
            return;
        }
        let mut saves: Vec<SaveState> = page_newest_first.into_iter().rev().collect();
        saves.append(&mut self.data.saves);
        self.data.saves = saves;
        if let Selection::Station(i) = &mut self.data.selection {
            *i += added;
        }
        // Again for every save: history order must hold across the page boundary too.
        self.place_saves();
    }

    /// Moves "Agora" to `now_local` (and a save dated after it along with it).
    pub fn set_now(&mut self, now_local: NaiveDateTime) {
        self.now_local = now_local;
        self.now_x = self.axis.x_of(now_local);
        self.place_saves();
    }

    /// Follows the widget's width, keeping what is at the right edge, where "Agora" usually is,
    /// in place. The view a temporary zoom goes back to follows too.
    pub fn set_width(&mut self, width: f64) {
        keep_right_edge(&mut self.view, width);
        if let Some(view) = &mut self.temp.return_to {
            keep_right_edge(view, width);
        }
    }

    /// The balls to draw: the saves near the view, grouped. "Você está aqui" and the selected
    /// save keep their own balls.
    pub fn items(&self) -> Vec<Item> {
        let pinned = self.pinned();
        let range = layout_range(&self.view, &self.xs);
        layout(&self.view, &self.xs, range, &pinned)
    }

    /// The screen x of "Agora": at its time, or [`NOW_MIN_GAP_PX`] right of "Você está aqui",
    /// whichever is further right.
    pub fn now_screen_x(&self) -> f64 {
        let here = self.data.you_are_here().and_then(|i| self.xs.get(i));
        let here_x = here.map(|&x| self.view.world_to_screen(x));
        axis::now_screen_x(&self.view, self.now_x, here_x)
    }

    /// The world x of each save, oldest first.
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

    /// What the pointer at (`x`, `y`), in widget coordinates, is on. "Você está aqui" and the
    /// selected save are painted on top, so they win anywhere inside their painted balls.
    pub fn hit_at(&self, x: f64, y: f64) -> Option<Hit> {
        hit_test(
            &self.items(),
            &self.pinned(),
            self.now_screen_x(),
            x,
            y,
            self.line_y,
        )
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

    /// Scrolls so that "Agora" sits near the right edge, keeping the zoom. This ends the
    /// temporary zoom.
    pub fn jump_to_end(&mut self) {
        self.temp.accept();
        self.put_now_at_end();
    }

    fn put_now_at_end(&mut self) {
        let dx = self.view.width - NOW_MARGIN_PX - self.now_screen_x();
        self.view.pan_by(dx);
    }

    fn place_saves(&mut self) {
        let placed = place_saves(&self.axis, &self.data.saves, self.to_local, self.now_local);
        self.oldest_dated = placed.iter().position(|p| p.dated);
        self.xs = placed.into_iter().map(|p| p.x).collect();
    }

    /// The saves that never join a group: "Você está aqui" and the selected save.
    fn pinned(&self) -> Vec<usize> {
        let mut pinned: Vec<usize> = self.data.you_are_here().into_iter().collect();
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
}

/// `view` at `width`, with the same world x at its right edge.
fn keep_right_edge(view: &mut View, width: f64) {
    view.left += (view.width - width) / view.day_px;
    view.width = width;
}

/// Whether the view is close enough to the oldest loaded save to load the page before it: more
/// saves exist, and the view's left edge is less than one screen width right of the oldest
/// loaded save with a readable date (the oldest save when none has one).
pub fn needs_older_page(s: &TimelineState) -> bool {
    let oldest = s.oldest_dated.unwrap_or(0);
    s.data.more_available
        && s.xs
            .get(oldest)
            .is_some_and(|&x| s.view.world_to_screen(x) > -s.view.width)
}

/// Applies an input event to the state and says what the owner has to do about it.
///
/// - The wheel zooms around the pointer, with or without Ctrl. Shift+wheel pans, and so does a
///   horizontal wheel.
/// - Dragging with the left button pans. A press and release that moved less than 4 px in all
///   is a click.
/// - A click on a save or "Agora" selects it. A click on a group zooms in on it temporarily, or
///   asks for its list when zooming in would not split it. A double click on "Agora" jumps to
///   the end.
/// - Wheel turns and drags end the temporary zoom; selecting does not.
/// - A change of the view or the temporary zoom gives `Redraw`, and a change of the view gives
///   `RequestOlder` when it needs an older page.
pub fn handle_event(s: &mut TimelineState, ev: &Event) -> Vec<TimelineAction> {
    let before = (s.view, s.temp);
    let mut actions = Vec::new();
    match *ev {
        Event::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            clicks,
        } => {
            s.drag = None;
            if clicks == 1 {
                s.drag = Some(0.0);
            } else if clicks == 2 && s.hit(x, y) == Some(Hit::Now) {
                s.jump_to_end();
                actions.extend([TimelineAction::JumpToEnd, TimelineAction::Redraw]);
            }
        }
        Event::MouseMove { dx, dy, .. } => {
            if let Some(moved) = s.drag.as_mut() {
                *moved += f64::from(dx).hypot(f64::from(dy));
                // Under the click slop it may still be a click (a peek keeps the temporary zoom).
                if *moved >= CLICK_SLOP_PX {
                    s.temp.accept();
                }
                pan(&mut s.view, f64::from(dx));
            }
        }
        Event::MouseUp {
            x,
            y,
            button: MouseButton::Left,
        } => {
            if let Some(moved) = s.drag.take()
                && moved < CLICK_SLOP_PX
                && let Some(hit) = s.hit(x, y)
            {
                actions = click(s, hit);
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
        Event::Leave => s.drag = None,
        _ => {}
    }
    finish(s, before, actions)
}

/// Esc: when a temporary zoom is active, goes back to the view from before it. Nothing
/// otherwise.
pub fn on_escape(s: &mut TimelineState) -> Vec<TimelineAction> {
    let before = (s.view, s.temp);
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

/// Ends `actions` with `Redraw` when the view or the temporary zoom changed since `before`, and
/// `RequestOlder` when the view changed and needs an older page.
fn finish(
    s: &TimelineState,
    before: (View, TempZoom),
    mut actions: Vec<TimelineAction>,
) -> Vec<TimelineAction> {
    let moved = s.view != before.0;
    let redraw = moved || s.temp != before.1;
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

    use TimelineAction::{Animate, JumpToEnd, OpenGroupList, Redraw, RequestOlder, Select};

    const WIDTH: f64 = 1000.0;
    const LINE_Y: f64 = 200.0;
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
        TimelineData {
            saves: times.iter().enumerate().map(|(n, &t)| save(n, t)).collect(),
            more_available: false,
            unsaved: 3,
            selection: Selection::Now,
            other_timelines: false,
        }
    }

    fn state(times: &[NaiveDateTime]) -> TimelineState {
        TimelineState::new(data(times), WIDTH, LINE_Y, own_offset, now())
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
        assert_eq!(s.line_y, LINE_Y);
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
    fn prepend_keeps_view_and_shifts_selection() {
        let mut s = spread();
        s.data.selection = Selection::Station(1);
        s.data.more_available = true;
        let view = s.view;
        let x1 = screen_x(&s, 1);
        // A page of two older saves, newest first.
        let page = vec![save(101, at(10, 31, 18, 0)), save(100, at(10, 31, 6, 0))];
        s.prepend_older(page, false);
        assert_eq!(s.view, view);
        assert_eq!(s.data.selection, Selection::Station(3));
        assert_eq!(screen_x(&s, 3), x1);
        assert!(!s.data.more_available);
        let names: Vec<&str> = s.data.saves.iter().map(|save| save.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Save 100", "Save 101", "Save 0", "Save 1", "Save 2", "Save 3", "Save 4"
            ]
        );
        assert_eq!(s.xs().len(), 7);
        assert_eq!(s.xs()[0], s.axis.x_of(at(10, 31, 6, 0)));

        // History order holds across the page boundary: a page save dated after the loaded
        // ones pushes them right, never left.
        s.prepend_older(vec![save(102, at(11, 1, 23, 0))], true);
        assert!(s.xs().windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(s.data.selection, Selection::Station(4));
        assert!(s.data.more_available);

        // An empty page only says whether more exist.
        let (xs, view) = (s.xs().to_vec(), s.view);
        s.prepend_older(Vec::new(), false);
        assert_eq!(s.xs(), xs);
        assert_eq!(s.view, view);
        assert!(!s.data.more_available);
        assert_eq!(s.data.selection, Selection::Station(4));
    }

    #[test]
    fn set_width_keeps_the_right_edge() {
        let mut s = grouped();
        let before = s.view;
        click_group(&mut s, 0..5);
        s.set_width(600.0);
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
    fn append_save_adds_the_newest_save_and_moves_the_clock() {
        let mut s = spread();
        s.data.selection = Selection::Station(2);
        let view = s.view;
        // A save made at 12:30, now that it is 12:31.
        s.append_save(save(5, at(11, 2, 12, 30)), at(11, 2, 12, 31));
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
