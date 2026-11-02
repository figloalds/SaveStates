//! Pure model of the timeline: stations on a line, a pannable and zoomable view, hit testing,
//! paging and input handling. Nothing here draws or calls Raven; the widget paints this state
//! and feeds it events.
//!
//! The line holds one station per save, oldest on the left, then the "Now" station for unsaved
//! changes. Station `i` sits at world x `i * STATION_SPACING`, and a [`View`] maps world x to
//! screen x as `(world_x - offset_x) * zoom`.

use std::ops::Range;

use savestates_core::SaveState;
use savestates_ui_raven::{Event, MouseButton};

/// World distance between two neighbouring stations.
pub const STATION_SPACING: f32 = 110.0;
pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 3.0;
/// Space kept to the right of the last station when jumping to the end.
pub const NOW_MARGIN_PX: f32 = 80.0;
/// How close (in screen pixels) a pointer must be to a station to hit it.
pub const HIT_RADIUS_PX: f32 = 12.0;

/// Load an older page once a station this close to the oldest loaded one is in view.
const OLDER_PAGE_TRIGGER: usize = 50;
/// A press and release that moved less than this (in pixels, in total) is a click, not a pan.
const CLICK_SLOP_PX: f32 = 4.0;
/// How far one wheel line pans, in screen pixels.
const WHEEL_PAN_PX: f32 = 60.0;
/// How much one wheel line zooms with Ctrl held.
const WHEEL_ZOOM_STEP: f32 = 1.15;

/// How much a station shows at a given zoom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    DotsOnly,
    Names,
    Full,
}

pub fn detail_for(zoom: f32) -> Detail {
    if zoom < 0.45 {
        Detail::DotsOnly
    } else if zoom < 0.9 {
        Detail::Names
    } else {
        Detail::Full
    }
}

/// The world x of station `i`.
pub fn station_x(i: usize) -> f32 {
    i as f32 * STATION_SPACING
}

/// What part of the line is on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Screen pixels per world unit.
    pub zoom: f32,
    /// The world x at the left edge of the widget.
    pub offset_x: f32,
    /// Widget width in screen pixels.
    pub width: f32,
}

impl View {
    pub fn world_to_screen(&self, wx: f32) -> f32 {
        (wx - self.offset_x) * self.zoom
    }

    pub fn screen_to_world(&self, sx: f32) -> f32 {
        sx / self.zoom + self.offset_x
    }

    /// Moves the content by `dx` screen pixels, as a drag does: positive is to the right.
    pub fn pan_by(&mut self, dx: f32) {
        if dx.is_finite() {
            self.offset_x -= dx / self.zoom;
        }
    }

    /// Multiplies the zoom by `factor` (clamped to the zoom limits), keeping the world point
    /// under screen x `anchor_x` where it is.
    pub fn zoom_at(&mut self, factor: f32, anchor_x: f32) {
        if !(factor.is_finite() && factor > 0.0 && anchor_x.is_finite()) {
            return;
        }
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        if zoom == self.zoom {
            return;
        }
        let anchor_world = self.screen_to_world(anchor_x);
        self.zoom = zoom;
        self.offset_x = anchor_world - anchor_x / zoom;
    }

    /// The stations worth painting: those on screen plus one spacing of margin on each side.
    pub fn visible_range(&self, count: usize) -> Range<usize> {
        let left = self.screen_to_world(0.0) - STATION_SPACING;
        let right = self.screen_to_world(self.width) + STATION_SPACING;
        // `as usize` saturates, so a negative or NaN position becomes 0.
        let end = (((right / STATION_SPACING).floor() + 1.0).max(0.0) as usize).min(count);
        let start = ((left / STATION_SPACING).ceil().max(0.0) as usize).min(end);
        start..end
    }

    /// Scrolls so that the last of `station_count` stations sits `NOW_MARGIN_PX` from the right
    /// edge.
    pub fn jump_to_end(&mut self, station_count: usize) {
        let last = station_x(station_count.saturating_sub(1));
        self.offset_x = last - (self.width - NOW_MARGIN_PX) / self.zoom;
    }

    /// Compensates for `added` stations inserted before the first one, so that every station
    /// that was on screen stays at the same screen x.
    pub fn shift_for_prepend(&mut self, added: usize) {
        self.offset_x += station_x(added);
    }
}

/// The station that is selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    /// The "Now" station: the unsaved changes.
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
    /// The saves plus the "Now" station.
    pub fn station_count(&self) -> usize {
        self.saves.len() + 1
    }

    /// The index of the "Now" station: right after the newest save.
    pub fn now_index(&self) -> usize {
        self.saves.len()
    }

    /// The index of the newest save, the one the project is at. `None` without saves.
    pub fn you_are_here(&self) -> Option<usize> {
        self.saves.len().checked_sub(1)
    }

    /// Adds a page of older saves, given newest first, before the loaded ones. The selection and
    /// `view` move with the stations, so that every station stays under the same screen x.
    ///
    /// It does not touch `more_available`: the caller knows whether that page was the last.
    pub fn prepend_older(&mut self, page_newest_first: Vec<SaveState>, view: &mut View) {
        let added = page_newest_first.len();
        if added == 0 {
            return;
        }
        let mut saves: Vec<SaveState> = page_newest_first.into_iter().rev().collect();
        saves.append(&mut self.saves);
        self.saves = saves;
        if let Selection::Station(i) = &mut self.selection {
            *i += added;
        }
        view.shift_for_prepend(added);
    }

    /// The selection a station index stands for.
    fn selection_at(&self, index: usize) -> Selection {
        if index == self.now_index() {
            Selection::Now
        } else {
            Selection::Station(index)
        }
    }
}

/// The station within `HIT_RADIUS_PX` of the pointer at (`x`, `y`), the nearest one if several
/// are. Stations all sit at `line_y`.
pub fn hit_test(view: &View, station_count: usize, x: f32, y: f32, line_y: f32) -> Option<usize> {
    let dy = y - line_y;
    let mut best: Option<(usize, f32)> = None;
    for i in view.visible_range(station_count) {
        let distance = (view.world_to_screen(station_x(i)) - x).hypot(dy);
        if distance <= HIT_RADIUS_PX && best.is_none_or(|(_, nearest)| distance < nearest) {
            best = Some((i, distance));
        }
    }
    best.map(|(i, _)| i)
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

/// Whether the view is close enough to the oldest loaded save to load the page before it.
pub fn needs_older_page(view: &View, data: &TimelineData) -> bool {
    data.more_available && view.visible_range(data.station_count()).start < OLDER_PAGE_TRIGGER
}

/// What the owner of a [`TimelineState`] has to do after an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineAction {
    /// A station was clicked. `data.selection` already holds it; repaint.
    Select(Selection),
    /// The user double-clicked "Now". The view is already at the end.
    JumpToEnd,
    /// Load the page of saves before the oldest one loaded.
    RequestOlder,
    /// The view changed; repaint.
    Redraw,
}

#[derive(Clone, Debug)]
pub struct TimelineState {
    pub data: TimelineData,
    pub view: View,
    /// The y of the line, in widget coordinates.
    pub line_y: f32,
    /// While the left button is held: how far the pointer has moved since it went down.
    drag: Option<f32>,
}

impl TimelineState {
    pub fn new(data: TimelineData, view: View, line_y: f32) -> Self {
        TimelineState {
            data,
            view,
            line_y,
            drag: None,
        }
    }
}

/// Applies an input event to the state and says what the owner has to do about it.
///
/// - Dragging with the left button pans. A press and release that barely moved is a click.
/// - The wheel pans, or zooms around the pointer with Ctrl held.
/// - A click on a station selects it; a double click on "Now" jumps to the end.
/// - A view change always gives `Redraw`, and `RequestOlder` when it needs an older page.
pub fn handle_event(s: &mut TimelineState, ev: &Event) -> Vec<TimelineAction> {
    let view_before = s.view;
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
            } else if clicks == 2
                && hit_test(&s.view, s.data.station_count(), x, y, s.line_y)
                    == Some(s.data.now_index())
            {
                s.view.jump_to_end(s.data.station_count());
                actions.push(TimelineAction::JumpToEnd);
            }
        }
        Event::MouseMove { dx, dy, .. } => {
            if let Some(moved) = s.drag.as_mut() {
                *moved += dx.hypot(dy);
                s.view.pan_by(dx);
            }
        }
        Event::MouseUp {
            x,
            y,
            button: MouseButton::Left,
        } => {
            if let Some(moved) = s.drag.take()
                && moved < CLICK_SLOP_PX
                && let Some(i) = hit_test(&s.view, s.data.station_count(), x, y, s.line_y)
            {
                let selection = s.data.selection_at(i);
                s.data.selection = selection;
                actions.push(TimelineAction::Select(selection));
            }
        }
        Event::Wheel { x, wy, ctrl, .. } => {
            if ctrl {
                s.view.zoom_at(WHEEL_ZOOM_STEP.powf(wy), x);
            } else {
                s.view.pan_by(wy * WHEEL_PAN_PX);
            }
        }
        Event::Leave => s.drag = None,
        _ => {}
    }
    if s.view != view_before {
        actions.push(TimelineAction::Redraw);
        if needs_older_page(&s.view, &s.data) {
            actions.push(TimelineAction::RequestOlder);
        }
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};
    use savestates_core::SaveId;
    use savestates_ui_raven::{Event, Key, MouseButton};

    const LINE_Y: f32 = 200.0;

    fn make_save(name: String, n: i64) -> SaveState {
        let time = FixedOffset::west_opt(3 * 3600)
            .unwrap()
            .timestamp_opt(1_700_000_000 + n * 60, 0)
            .unwrap();
        SaveState {
            id: SaveId(format!("{n:040x}")),
            name,
            time,
            committed: time,
            files_changed: 1,
        }
    }

    fn save(i: usize) -> SaveState {
        make_save(format!("Save {i}"), i as i64)
    }

    fn data(saves: usize) -> TimelineData {
        TimelineData {
            saves: (0..saves).map(save).collect(),
            more_available: false,
            unsaved: 3,
            selection: Selection::Now,
            other_timelines: false,
        }
    }

    fn state(saves: usize) -> TimelineState {
        let view = View {
            zoom: 1.0,
            offset_x: 0.0,
            width: 1500.0,
        };
        TimelineState::new(data(saves), view, LINE_Y)
    }

    fn assert_close(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-3, "{a} is not close to {b}");
    }

    fn down(x: f32, y: f32, clicks: i32) -> Event {
        Event::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            clicks,
        }
    }

    fn up(x: f32, y: f32) -> Event {
        Event::MouseUp {
            x,
            y,
            button: MouseButton::Left,
        }
    }

    fn mv(x: f32, y: f32, dx: f32, dy: f32) -> Event {
        Event::MouseMove { x, y, dx, dy }
    }

    fn wheel(x: f32, wy: f32, ctrl: bool) -> Event {
        Event::Wheel {
            x,
            y: LINE_Y,
            wx: 0.0,
            wy,
            ctrl,
        }
    }

    #[test]
    fn round_trip() {
        for zoom in [0.3, 2.0] {
            let v = View {
                zoom,
                offset_x: 123.5,
                width: 800.0,
            };
            for x in [-50.0, 0.0, 37.25, 799.0, 5_000.0] {
                assert_close(v.screen_to_world(v.world_to_screen(x)), x);
            }
        }
    }

    #[test]
    fn world_to_screen_scales_then_offsets() {
        let v = View {
            zoom: 2.0,
            offset_x: 100.0,
            width: 800.0,
        };
        assert_close(v.world_to_screen(100.0), 0.0);
        assert_close(v.world_to_screen(150.0), 100.0);
        assert_close(v.screen_to_world(100.0), 150.0);
    }

    #[test]
    fn zoom_keeps_anchor() {
        let mut v = View {
            zoom: 1.0,
            offset_x: 250.0,
            width: 1000.0,
        };
        let before = v.screen_to_world(300.0);
        v.zoom_at(1.5, 300.0);
        assert_close(v.zoom, 1.5);
        assert_close(v.screen_to_world(300.0), before);
        assert_close(v.world_to_screen(before), 300.0);
    }

    #[test]
    fn zoom_clamps() {
        let mut v = View {
            zoom: 1.0,
            offset_x: 0.0,
            width: 1000.0,
        };
        for _ in 0..40 {
            v.zoom_at(1.5, 400.0);
            assert!((MIN_ZOOM..=MAX_ZOOM).contains(&v.zoom));
        }
        assert_eq!(v.zoom, MAX_ZOOM);
        for _ in 0..80 {
            v.zoom_at(0.5, 400.0);
            assert!((MIN_ZOOM..=MAX_ZOOM).contains(&v.zoom));
        }
        assert_eq!(v.zoom, MIN_ZOOM);
    }

    #[test]
    fn zoom_ignores_nonsense_factors() {
        let mut v = View {
            zoom: 1.0,
            offset_x: 10.0,
            width: 1000.0,
        };
        let before = v;
        for factor in [f32::NAN, f32::INFINITY, 0.0, -2.0] {
            v.zoom_at(factor, 300.0);
            assert_eq!(v, before);
        }
    }

    #[test]
    fn pan_moves_content_with_the_pointer() {
        let mut v = View {
            zoom: 2.0,
            offset_x: 100.0,
            width: 1000.0,
        };
        v.pan_by(50.0);
        assert_close(v.offset_x, 75.0);
        v.pan_by(f32::NAN);
        assert_close(v.offset_x, 75.0);
    }

    #[test]
    fn visible_range_concrete() {
        let v = View {
            zoom: 1.0,
            offset_x: 0.0,
            width: 1000.0,
        };
        assert_eq!(v.visible_range(10_000), 0..11);
    }

    #[test]
    fn visible_range_follows_the_view_and_clamps_to_the_count() {
        let mut v = View {
            zoom: 1.0,
            offset_x: 5_500.0,
            width: 1000.0,
        };
        assert_eq!(v.visible_range(10_000), 49..61);
        assert_eq!(v.visible_range(55), 49..55);

        v.zoom = 0.5;
        // 2,000 world units on screen plus a spacing each side: 5,390 / 110 to 7,610 / 110.
        assert_eq!(v.visible_range(10_000), 49..70);

        v.offset_x = 0.0;
        assert_eq!(v.visible_range(5), 0..5);
        assert_eq!(v.visible_range(0), 0..0);
    }

    #[test]
    fn visible_range_is_empty_when_scrolled_past_either_end() {
        let mut v = View {
            zoom: 1.0,
            offset_x: 10_000.0,
            width: 1000.0,
        };
        assert!(v.visible_range(10).is_empty());
        v.offset_x = -5_000.0;
        assert!(v.visible_range(10).is_empty());
    }

    #[test]
    fn jump_to_end_places_now() {
        let mut v = View {
            zoom: 1.0,
            offset_x: 0.0,
            width: 800.0,
        };
        v.jump_to_end(6);
        assert_close(v.world_to_screen(station_x(5)), 720.0);

        v.zoom = 0.5;
        v.jump_to_end(6);
        assert_close(v.world_to_screen(station_x(5)), 720.0);

        v.jump_to_end(0);
        assert_close(v.world_to_screen(station_x(0)), 720.0);
    }

    #[test]
    fn hit_test_radius() {
        let v = View {
            zoom: 1.0,
            offset_x: 0.0,
            width: 1500.0,
        };
        let d = data(10);
        let count = d.station_count();
        let x3 = station_x(3);
        assert_eq!(hit_test(&v, count, x3 + 10.0, LINE_Y, LINE_Y), Some(3));
        assert_eq!(hit_test(&v, count, x3 - 10.0, LINE_Y, LINE_Y), Some(3));
        assert_eq!(hit_test(&v, count, x3 + 20.0, LINE_Y, LINE_Y), None);
        assert_eq!(hit_test(&v, count, x3 - 20.0, LINE_Y, LINE_Y), None);
        // The Now station comes right after the last save.
        assert_eq!(
            hit_test(&v, count, station_x(d.saves.len()), LINE_Y, LINE_Y),
            Some(d.saves.len())
        );
        assert_eq!(hit_test(&v, 0, 0.0, LINE_Y, LINE_Y), None);
    }

    #[test]
    fn hit_test_uses_euclidean_distance() {
        let v = View {
            zoom: 1.0,
            offset_x: 0.0,
            width: 1500.0,
        };
        let x3 = station_x(3);
        assert_eq!(hit_test(&v, 11, x3, LINE_Y + 10.0, LINE_Y), Some(3));
        assert_eq!(hit_test(&v, 11, x3, LINE_Y - 13.0, LINE_Y), None);
        // 9 px right and 9 px down is 12.7 px away, outside the 12 px radius.
        assert_eq!(hit_test(&v, 11, x3 + 9.0, LINE_Y + 9.0, LINE_Y), None);
        assert_eq!(hit_test(&v, 11, x3 + 8.0, LINE_Y + 8.0, LINE_Y), Some(3));
    }

    #[test]
    fn hit_test_picks_the_nearest_when_stations_are_close() {
        // At zoom 0.1 the stations are 11 px apart, so both 3 (33 px) and 4 (44 px) qualify.
        let v = View {
            zoom: 0.1,
            offset_x: 0.0,
            width: 1500.0,
        };
        assert_eq!(hit_test(&v, 11, 40.0, LINE_Y, LINE_Y), Some(4));
        assert_eq!(hit_test(&v, 11, 36.0, LINE_Y, LINE_Y), Some(3));
    }

    #[test]
    fn prepend_keeps_screen_position() {
        let mut d = data(30);
        d.selection = Selection::Station(5);
        let mut v = View {
            zoom: 0.5,
            offset_x: 100.0,
            width: 1500.0,
        };
        let before = v.world_to_screen(station_x(5));
        let page: Vec<SaveState> = (0..500)
            .map(|i| make_save(format!("Old {i}"), -1 - i as i64))
            .collect();

        d.prepend_older(page, &mut v);

        assert_eq!(d.saves.len(), 530);
        assert_eq!(d.selection, Selection::Station(505));
        assert_close(v.world_to_screen(station_x(505)), before);
        // The page arrives newest first; the timeline runs oldest first.
        assert_eq!(d.saves[0].name, "Old 499");
        assert_eq!(d.saves[499].name, "Old 0");
        assert_eq!(d.saves[500].name, "Save 0");
        assert_eq!(d.saves[529].name, "Save 29");
    }

    #[test]
    fn prepend_leaves_now_selected_and_ignores_an_empty_page() {
        let mut d = data(3);
        let mut v = View {
            zoom: 1.0,
            offset_x: 40.0,
            width: 800.0,
        };
        d.prepend_older(vec![save(100), save(101)], &mut v);
        assert_eq!(d.selection, Selection::Now);
        assert_eq!(d.saves.len(), 5);
        assert_close(v.offset_x, 40.0 + 2.0 * STATION_SPACING);

        let (saves, view) = (d.saves.clone(), v);
        d.prepend_older(Vec::new(), &mut v);
        assert_eq!(d.saves, saves);
        assert_eq!(v, view);
    }

    #[test]
    fn station_indexes() {
        let mut d = data(0);
        assert_eq!(d.station_count(), 1);
        assert_eq!(d.now_index(), 0);
        assert_eq!(d.you_are_here(), None);
        d = data(4);
        assert_eq!(d.station_count(), 5);
        assert_eq!(d.now_index(), 4);
        assert_eq!(d.you_are_here(), Some(3));
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

    #[test]
    fn detail_thresholds() {
        assert_eq!(detail_for(0.3), Detail::DotsOnly);
        assert_eq!(detail_for(0.6), Detail::Names);
        assert_eq!(detail_for(1.0), Detail::Full);
        assert_eq!(detail_for(MIN_ZOOM), Detail::DotsOnly);
        assert_eq!(detail_for(0.45), Detail::Names);
        assert_eq!(detail_for(0.9), Detail::Full);
        assert_eq!(detail_for(MAX_ZOOM), Detail::Full);
    }

    #[test]
    fn older_page_is_needed_near_the_oldest_loaded_station() {
        let mut d = data(200);
        let mut v = View {
            zoom: 1.0,
            offset_x: 5_610.0,
            width: 1000.0,
        };
        // Station 50 is the first one in view: not near enough yet.
        assert_eq!(v.visible_range(d.station_count()).start, 50);
        d.more_available = true;
        assert!(!needs_older_page(&v, &d));
        v.offset_x = 5_500.0;
        assert_eq!(v.visible_range(d.station_count()).start, 49);
        assert!(needs_older_page(&v, &d));
        d.more_available = false;
        assert!(!needs_older_page(&v, &d));
    }

    #[test]
    fn events_drag_pans_by_screen_distance() {
        for zoom in [0.5, 1.0, 2.0] {
            let mut s = state(10);
            s.view = View {
                zoom,
                offset_x: 1_000.0,
                width: 1500.0,
            };
            // Moving without a pressed button does nothing.
            assert!(handle_event(&mut s, &mv(300.0, LINE_Y, 20.0, 0.0)).is_empty());
            assert_close(s.view.offset_x, 1_000.0);

            assert!(handle_event(&mut s, &down(300.0, LINE_Y, 1)).is_empty());
            let actions = handle_event(&mut s, &mv(350.0, LINE_Y, 50.0, 0.0));
            assert_eq!(actions, vec![TimelineAction::Redraw]);
            assert_close(s.view.offset_x, 1_000.0 - 50.0 / zoom);
            assert_eq!(s.view.zoom, zoom);
            // Letting go after a pan is not a click.
            assert!(handle_event(&mut s, &up(350.0, LINE_Y)).is_empty());
            assert_eq!(s.data.selection, Selection::Now);
            // And the drag is over.
            assert!(handle_event(&mut s, &mv(400.0, LINE_Y, 50.0, 0.0)).is_empty());
        }
    }

    #[test]
    fn events_wheel_pans_sixty_pixels_per_line() {
        let mut s = state(10);
        s.view.offset_x = 1_000.0;
        let actions = handle_event(&mut s, &wheel(300.0, 2.0, false));
        assert_eq!(actions, vec![TimelineAction::Redraw]);
        assert_close(s.view.offset_x, 1_000.0 - 120.0);
        handle_event(&mut s, &wheel(300.0, -1.0, false));
        assert_close(s.view.offset_x, 1_000.0 - 120.0 + 60.0);
        assert_eq!(s.view.zoom, 1.0);
    }

    #[test]
    fn events_ctrl_wheel_zooms_at_the_pointer() {
        let mut s = state(10);
        s.view.offset_x = 250.0;
        let anchored = s.view.screen_to_world(300.0);

        let actions = handle_event(&mut s, &wheel(300.0, 1.0, true));
        assert_eq!(actions, vec![TimelineAction::Redraw]);
        assert_close(s.view.zoom, 1.15);
        assert_close(s.view.screen_to_world(300.0), anchored);

        handle_event(&mut s, &wheel(300.0, -1.0, true));
        assert_close(s.view.zoom, 1.0);

        // Nothing changes at a zoom limit, so nothing needs redrawing, not even a rounding error
        // in the offset (these values are not exact in f32).
        for (zoom, wy) in [(MAX_ZOOM, 1.0), (MIN_ZOOM, -1.0)] {
            s.view = View {
                zoom,
                offset_x: 123.456,
                width: 1500.0,
            };
            let before = s.view;
            assert!(handle_event(&mut s, &wheel(301.3, wy, true)).is_empty());
            assert_eq!(s.view, before);
        }
    }

    #[test]
    fn events_click_selects_a_station() {
        let mut s = state(10);
        let x2 = station_x(2);
        assert!(handle_event(&mut s, &down(x2 + 2.0, LINE_Y + 1.0, 1)).is_empty());
        let actions = handle_event(&mut s, &up(x2 + 2.0, LINE_Y + 1.0));
        assert_eq!(actions, vec![TimelineAction::Select(Selection::Station(2))]);
        assert_eq!(s.data.selection, Selection::Station(2));

        // The Now station selects Now.
        let now_x = station_x(s.data.now_index());
        handle_event(&mut s, &down(now_x, LINE_Y, 1));
        let actions = handle_event(&mut s, &up(now_x, LINE_Y));
        assert_eq!(actions, vec![TimelineAction::Select(Selection::Now)]);
        assert_eq!(s.data.selection, Selection::Now);
    }

    #[test]
    fn events_click_on_empty_space_or_with_another_button_selects_nothing() {
        let mut s = state(10);
        s.data.selection = Selection::Station(4);
        // Halfway between stations 2 and 3.
        let gap = (station_x(2) + station_x(3)) / 2.0;
        handle_event(&mut s, &down(gap, LINE_Y, 1));
        assert!(handle_event(&mut s, &up(gap, LINE_Y)).is_empty());

        let x2 = station_x(2);
        let right_down = Event::MouseDown {
            x: x2,
            y: LINE_Y,
            button: MouseButton::Right,
            clicks: 1,
        };
        let right_up = Event::MouseUp {
            x: x2,
            y: LINE_Y,
            button: MouseButton::Right,
        };
        assert!(handle_event(&mut s, &right_down).is_empty());
        assert!(handle_event(&mut s, &right_up).is_empty());
        assert_eq!(s.data.selection, Selection::Station(4));
    }

    #[test]
    fn events_a_small_wobble_is_still_a_click_but_a_real_drag_is_not() {
        let x2 = station_x(2);

        let mut s = state(10);
        handle_event(&mut s, &down(x2, LINE_Y, 1));
        handle_event(&mut s, &mv(x2 + 3.0, LINE_Y, 3.0, 0.0));
        let actions = handle_event(&mut s, &up(x2 + 3.0, LINE_Y));
        assert_eq!(actions, vec![TimelineAction::Select(Selection::Station(2))]);

        // Several small moves add up.
        let mut s = state(10);
        handle_event(&mut s, &down(x2, LINE_Y, 1));
        for step in 1..=3 {
            let x = x2 + 2.0 * step as f32;
            handle_event(&mut s, &mv(x, LINE_Y, 2.0, 0.0));
        }
        assert!(handle_event(&mut s, &up(x2 + 6.0, LINE_Y)).is_empty());
        assert_eq!(s.data.selection, Selection::Now);

        // Movement in y counts too.
        let mut s = state(10);
        handle_event(&mut s, &down(x2, LINE_Y, 1));
        handle_event(&mut s, &mv(x2, LINE_Y + 5.0, 0.0, 5.0));
        assert!(handle_event(&mut s, &up(x2, LINE_Y)).is_empty());
    }

    #[test]
    fn events_double_click_on_now_jumps_to_the_end() {
        let mut s = state(10);
        let now_x = station_x(s.data.now_index());
        let actions = handle_event(&mut s, &down(now_x, LINE_Y, 2));
        assert_eq!(
            actions,
            vec![TimelineAction::JumpToEnd, TimelineAction::Redraw]
        );
        let last = s.data.station_count() - 1;
        let x = s.view.width - NOW_MARGIN_PX;
        assert_close(s.view.world_to_screen(station_x(last)), x);
        // Letting go afterwards is not a click.
        assert!(handle_event(&mut s, &up(x, LINE_Y)).is_empty());

        // Already there: still a jump, but the view did not change.
        let actions = handle_event(&mut s, &down(x, LINE_Y, 2));
        assert_eq!(actions, vec![TimelineAction::JumpToEnd]);
    }

    #[test]
    fn events_double_click_elsewhere_does_nothing() {
        let mut s = state(10);
        let before = s.view;
        let x2 = station_x(2);
        assert!(handle_event(&mut s, &down(x2, LINE_Y, 2)).is_empty());
        assert!(handle_event(&mut s, &mv(x2 + 50.0, LINE_Y, 50.0, 0.0)).is_empty());
        assert!(handle_event(&mut s, &up(x2 + 50.0, LINE_Y)).is_empty());
        assert_eq!(s.view, before);
        assert_eq!(s.data.selection, Selection::Now);
    }

    #[test]
    fn events_leave_cancels_a_drag() {
        let mut s = state(10);
        s.view.offset_x = 500.0;
        handle_event(&mut s, &down(300.0, LINE_Y, 1));
        handle_event(&mut s, &mv(310.0, LINE_Y, 10.0, 0.0));
        assert!(handle_event(&mut s, &Event::Leave).is_empty());
        let offset = s.view.offset_x;
        assert!(handle_event(&mut s, &mv(400.0, LINE_Y, 90.0, 0.0)).is_empty());
        assert_close(s.view.offset_x, offset);
        assert!(handle_event(&mut s, &up(400.0, LINE_Y)).is_empty());
    }

    #[test]
    fn events_other_events_are_ignored() {
        let mut s = state(10);
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
    fn events_panning_near_the_oldest_loaded_station_requests_an_older_page() {
        let drag_to_the_past = |more_available: bool, dx: f32| {
            let mut s = state(200);
            s.data.more_available = more_available;
            s.view = View {
                zoom: 1.0,
                offset_x: 11_000.0,
                width: 1000.0,
            };
            handle_event(&mut s, &down(300.0, LINE_Y, 1));
            handle_event(&mut s, &mv(300.0 + dx, LINE_Y, dx, 0.0))
        };

        // 50 stations to the left puts station 49 in view.
        let near = drag_to_the_past(true, 5_500.0);
        assert_eq!(
            near,
            vec![TimelineAction::Redraw, TimelineAction::RequestOlder]
        );
        // The same pan with nothing more to load only redraws.
        assert_eq!(
            drag_to_the_past(false, 5_500.0),
            vec![TimelineAction::Redraw]
        );
        // A short pan stays far from the oldest station.
        assert_eq!(drag_to_the_past(true, 100.0), vec![TimelineAction::Redraw]);
    }

    #[test]
    fn events_zooming_out_can_request_an_older_page() {
        let mut s = state(200);
        s.data.more_available = true;
        s.view = View {
            zoom: 1.0,
            offset_x: 6_000.0,
            width: 1000.0,
        };
        // Zooming out around the right edge pulls older stations into view on the left.
        let actions = handle_event(&mut s, &wheel(1_000.0, -8.0, true));
        assert_eq!(
            actions,
            vec![TimelineAction::Redraw, TimelineAction::RequestOlder]
        );
    }
}
