//! Painting the timeline: one [`Scene`] per paint, from the model's state and what the widget
//! adds to it (the pointer and its tooltip, the save growing in, a group's list), over the day
//! grid (spec §4.2, §4.4–§4.8).
//!
//! Also the pure pieces the painting needs: the grid's labels, a group's tooltip text, and where
//! a group's list goes (the widget hit-tests the list with the same [`ListBox`]).

use std::f32::consts::{FRAC_PI_2, TAU};
use std::ops::Range;
use std::time::Instant;

use chrono::{Datelike, NaiveDate, NaiveTime, TimeDelta};
use savestates_core::SaveState;
use savestates_i18n::{
    Lang, day_label, day_label_full, hour_label, minute_label, month_label, month_name,
    relative_date_local, short_date, time_label, week_start_label,
};
use savestates_ui_raven::{Painter, Rgba, text_width, tr, tr_fmt, tr_plural};

use super::axis::{GRID_MIN_SPACING_PX, GridLabel, GridLine, GridStep, grid_lines, grid_step};
use super::layout::{Hit, Item};
use super::model::{Selection, TimelineData, TimelineState, truncate_label};
use crate::{theme, tr_key};

/// The size the lanes' names are measured at, for their name boxes.
pub(super) const LANE_NAME_SIZE: f32 = 11.0;
/// The saved line: a wide faint pass for the glow, then the line itself.
const GLOW_WIDTH: f32 = 6.0;
const GLOW_ALPHA: f32 = 0.25;
const LINE_WIDTH: f32 = 3.0;
/// How far past the widget's edges lines are drawn, so their round caps never show there.
const OVERDRAW: f32 = 10.0;

/// A save: a ring around a dark center that hides the line.
const STATION_R: f32 = 7.0;
const STATION_STROKE: f32 = 2.5;
/// "You are here": a filled dot inside a faint ring.
const HERE_R: f32 = 9.0;
const HERE_RING_R: f32 = 15.0;
const HERE_RING_STROKE: f32 = 2.5;
const HERE_RING_ALPHA: f32 = 0.35;
/// "Now": a dashed amber circle, dimmed when nothing changed.
const NOW_R: f32 = 11.0;
const NOW_DASHES: usize = 12;
/// The part of each of the 12 steps around the circle that is drawn.
const NOW_DASH_FILL: f32 = 0.6;
const NOW_STROKE: f32 = 2.0;
const NOW_DIM_ALPHA: f32 = 0.4;
/// The dashed connector from "You are here" to "Now".
const CONNECTOR_DASH: f32 = 4.0;
const CONNECTOR_GAP: f32 = 4.0;
const CONNECTOR_WIDTH: f32 = 2.0;
/// The halo of the selected station, and the softer one under the pointer.
const HALO_EXTRA: f32 = 7.0;
const HALO_ALPHA: f32 = 0.22;
const HALO_RING_EXTRA: f32 = 4.0;
const HALO_RING_ALPHA: f32 = 0.7;
const HOVER_EXTRA: f32 = 5.0;
const HOVER_ALPHA: f32 = 0.10;
/// A group: a ball tinted cyan (`#13284a` over the background), a cyan ring, the count inside.
const GROUP_FILL_ALPHA: f32 = 0.16;
const GROUP_STROKE: f32 = 2.0;

/// The background grid: thin lines (`#1b2440`-like), today's column softly tinted, and labels
/// at the top.
const GRID_LINE_WIDTH: f32 = 1.0;
const GRID_LINE_ALPHA: f32 = 0.5;
const TODAY_ALPHA: f32 = 0.05;
const GRID_SIZE: f32 = 10.5;
const GRID_LABEL_TOP: f32 = 6.0;
/// A grid label starts this far right of its line.
const GRID_LABEL_PAD: f32 = 5.0;
/// The least room between two grid labels: a label closer to the one before it is skipped.
const GRID_LABEL_GAP: f32 = 8.0;
/// The length of a month in days, on average, for spacing the month labels.
const DAYS_PER_MONTH: f64 = 365.25 / 12.0;
/// Month labels go on every month, or every 2nd, 3rd, 6th or 12th one (from January).
const MONTH_STRIDES: [u32; 5] = [1, 2, 3, 6, 12];

/// Names are cut to this many characters, and further when the room between stations is less.
const LABEL_CHARS: usize = 24;
/// Space kept between the labels of neighbouring stations.
const LABEL_GAP: f32 = 4.0;
/// A label narrower than this is not drawn at all.
const MIN_LABEL_W: f32 = 18.0;
/// The labels start this far below the line.
const LABEL_TOP: f32 = HERE_RING_R + 7.0;
/// Labels of balls further than this off the widget's edges can't show: they are not measured.
const LABEL_REACH: f32 = 100.0;
/// "Now"'s label may spread this far left of the station when no label is beside it.
const NOW_LABEL_HALF: f32 = 60.0;
/// "Now"'s explanation wraps at this width.
const NOW_DETAIL_W: f32 = 150.0;
/// Space kept from the widget's edges.
const EDGE_PAD: f32 = 6.0;
const NAME_SIZE: f32 = 12.0;
const DETAIL_SIZE: f32 = 11.0;
const TAG_SIZE: f32 = 10.0;
const COUNT_SIZE: f32 = 11.0;
/// The height of a line of text, per unit of font size (the theme font, Roboto).
const LINE_HEIGHT: f32 = 1.25;
/// Between a name and a time ("Modo escuro · 15:02"), and between the two ends of a span of
/// time ("13:05 – 18:40").
const SEPARATOR: &str = " · ";
const SPAN_DASH: &str = " – ";

/// The tooltip, next to the pointer.
const TOOLTIP_MAX_W: f32 = 320.0;
const TOOLTIP_NAME_LINES: usize = 3;
const TOOLTIP_PAD_X: f32 = 8.0;
const TOOLTIP_PAD_Y: f32 = 6.0;
const TOOLTIP_RADIUS: f32 = 6.0;

/// The pill shown during a temporary zoom, at the bottom right.
const PILL_SIZE: f32 = 11.0;
const PILL_PAD_X: f32 = 12.0;
const PILL_PAD_Y: f32 = 5.0;

/// A group's list: a box of rows ("name · time") under the group's ball.
const LIST_W: f32 = 300.0;
const LIST_ROW_H: f32 = 20.0;
/// Space inside the box above the first row and below the last.
const LIST_PAD: f32 = 4.0;
/// Space between the box's sides and the rows' text.
const LIST_TEXT_PAD: f32 = 10.0;
/// Space between the group's ball and the box.
const LIST_GAP: f32 = 8.0;
/// Under the ball when at least this many rows fit there (or every row, if fewer); otherwise
/// the box moves up as far as needed.
const LIST_MIN_ROWS: usize = 3;
const LIST_RADIUS: f32 = 6.0;
const LIST_SCROLLBAR_W: f32 = 3.0;
/// Wheel turns this close to a whole number of lines count as that number.
const WHEEL_SLACK: f32 = 1e-3;

/// A new save grows in over this long, easing out.
pub(super) const GROW_S: f32 = 0.25;

/// What the widget shows besides the model's state.
pub(super) struct Look<'a> {
    /// The language of the dates.
    pub lang: Lang,
    /// What is under the pointer, unless a button is held.
    pub hover: Option<&'a Hit>,
    /// Where the pointer is, while it is over the widget.
    pub pointer: Option<(f32, f32)>,
    /// The tooltip of what is under the pointer is due.
    pub tooltip: bool,
    /// The save growing in after a Quick Save, and when it started.
    pub grow: Option<(usize, Instant)>,
    /// The list of a group's saves, while it is open.
    pub list: Option<&'a GroupList>,
}

/// One paint of the timeline: what the state looks like at this size and moment.
pub(super) struct Scene<'a> {
    look: &'a Look<'a>,
    state: &'a TimelineState,
    data: &'a TimelineData,
    w: f32,
    h: f32,
    /// The y of "Now"'s line.
    now_y: f32,
    /// The balls near the view, lane by lane, in index order.
    items: Vec<Item>,
    here: Option<usize>,
    /// The selected save, when a save is selected.
    selected: Option<usize>,
    /// The screen x of "Now".
    now_x: f32,
    hover: Option<&'a Hit>,
    /// The grid's step at this zoom.
    step: GridStep,
    /// The local day "Agora" is in.
    today: NaiveDate,
    clock: Instant,
}

impl<'a> Scene<'a> {
    pub(super) fn new(state: &'a TimelineState, look: &'a Look<'a>, w: f32, h: f32) -> Scene<'a> {
        let selected = match state.data.selection {
            Selection::Station(i) => Some(i),
            Selection::Now => None,
        };
        Scene {
            look,
            state,
            data: &state.data,
            w,
            h,
            now_y: state.now_line_y() as f32,
            items: state.items(),
            here: state.data.you_are_here(),
            selected,
            now_x: state.now_screen_x() as f32,
            hover: look.hover,
            step: grid_step(state.view.day_px),
            today: state.now_local().date(),
            clock: Instant::now(),
        }
    }

    pub(super) fn paint(&self, p: &mut Painter) {
        self.today_column(p);
        self.grid(p);
        self.line(p);
        self.connector(p);
        self.balls(p);
        self.selected_station(p);
        self.here_station(p);
        self.now_station(p);
        self.labels(p);
        self.now_label(p);
        self.here_label(p);
        self.pill(p);
        self.tooltip(p);
        self.group_list(p);
    }

    /// The screen x of save `i`.
    fn x(&self, i: usize) -> f32 {
        let state = self.state;
        state
            .xs()
            .get(i)
            .map_or(f32::NAN, |&x| state.view.world_to_screen(x) as f32)
    }

    /// The y of the line save `i` is drawn on: its lane's.
    fn y(&self, i: usize) -> f32 {
        self.state
            .lane_of(i)
            .map_or(self.now_y, |lane| self.state.lane_line_y(lane) as f32)
    }

    /// The y of `item`'s ball: its lane's line.
    fn item_y(&self, item: &Item) -> f32 {
        self.y(first_save(item))
    }

    /// Whether save `i` keeps its own ball, painted on top: "You are here" or the selected save.
    fn is_pinned(&self, i: usize) -> bool {
        Some(i) == self.here || Some(i) == self.selected
    }

    /// The screen x of save `i`'s own ball, when it has one near the view.
    fn single_x(&self, i: usize) -> Option<f32> {
        self.items.iter().find_map(|item| match *item {
            Item::Single { index, x } if index == i => Some(x as f32),
            _ => None,
        })
    }

    /// "You are here", when it has a ball near the view.
    fn here_in_view(&self) -> Option<usize> {
        let here = self.here?;
        self.single_x(here).map(|_| here)
    }

    /// Whether "Now" or its label can show.
    fn now_in_view(&self) -> bool {
        let margin = 2.0 * NOW_LABEL_HALF;
        (-margin..self.w + margin).contains(&self.now_x)
    }

    /// How far station `i` has grown in, from 0 to 1.
    fn grown(&self, i: usize) -> f32 {
        match self.look.grow {
            Some((index, start)) if index == i => {
                let t = self.clock.saturating_duration_since(start).as_secs_f32() / GROW_S;
                ease_out(t)
            }
            _ => 1.0,
        }
    }

    fn now_alpha(&self) -> f32 {
        if self.data.unsaved == 0 {
            NOW_DIM_ALPHA
        } else {
            1.0
        }
    }

    /// When save `save` was made, as its label says: the time, with the date when the grid
    /// shows weeks or months.
    fn when(&self, save: &SaveState) -> String {
        let local = self.state.local(save.committed);
        let time = time_label(local.time());
        match self.step {
            GridStep::Weeks | GridStep::Months => {
                format!("{} {time}", short_date(self.look.lang, local.date()))
            }
            _ => time,
        }
    }

    /// The times of the first and the last save of `members` ("13:05 – 18:40"), or one time
    /// when they are the same.
    fn time_span(&self, members: &Range<usize>) -> String {
        let time = |i: usize| {
            let save = self.data.saves.get(i)?;
            Some(time_label(self.state.local(save.committed).time()))
        };
        let last = members.end.checked_sub(1).and_then(time);
        match (time(members.start), last) {
            (Some(first), Some(last)) if first != last => format!("{first}{SPAN_DASH}{last}"),
            (Some(first), _) => first,
            (None, _) => String::new(),
        }
    }

    fn save_tooltip_lines(&self, save: &SaveState) -> Vec<(String, f32, Rgba)> {
        let measure = |s: &str| text_width(NAME_SIZE, s);
        let width = TOOLTIP_MAX_W - 2.0 * TOOLTIP_PAD_X;
        let mut lines: Vec<(String, f32, Rgba)> =
            wrap_lines(&save.name, width, TOOLTIP_NAME_LINES, measure)
                .into_iter()
                .map(|line| (line, NAME_SIZE, theme::TEXT))
                .collect();
        let files = tr_plural("timeline.files", i64::from(save.files_changed));
        lines.push((self.tooltip_when(save), DETAIL_SIZE, theme::MUTED_TEXT));
        lines.push((files, DETAIL_SIZE, theme::MUTED_TEXT));
        lines
    }

    /// When `save` was made, as its tooltip says: the date as the rest of the app says it, and
    /// the time when that leaves it out. In the local time the save is placed and labeled at.
    fn tooltip_when(&self, save: &SaveState) -> String {
        let at = self.state.local(save.committed);
        let now = self.state.now_local();
        let date = relative_date_local(self.look.lang, at, now);
        if at.date() == now.date() {
            date
        } else {
            format!("{date} {}", time_label(at.time()))
        }
    }

    fn group_tooltip_lines(&self, members: &Range<usize>) -> Option<Vec<(String, f32, Rgba)>> {
        let day = |i: usize| {
            let save = self.data.saves.get(i)?;
            Some(self.state.local(save.committed).date())
        };
        let first = day(members.start)?;
        let last = day(members.end.checked_sub(1)?)?;
        let title = group_title(self.look.lang, members.len(), first, last);
        let args: Vec<&str> = title.args.iter().map(String::as_str).collect();
        let hint = group_hint(self.state.group_splits(members.clone()));
        Some(vec![
            (tr_fmt(title.key, &args), NAME_SIZE, theme::TEXT),
            (tr(hint), DETAIL_SIZE, theme::MUTED_TEXT),
        ])
    }

    fn now_tooltip_lines(&self) -> Vec<(String, f32, Rgba)> {
        let detail = if self.data.unsaved == 0 {
            tr("timeline.nothing_new")
        } else {
            tr_plural("changes.unsaved", self.data.unsaved as i64)
        };
        vec![
            (tr("timeline.now"), NAME_SIZE, theme::UNSAVED),
            (detail, DETAIL_SIZE, theme::MUTED_TEXT),
        ]
    }

    /// Today's column, softly tinted.
    fn today_column(&self, p: &mut Painter) {
        let view = &self.state.view;
        let start = self.state.axis.x_of(self.today.and_time(NaiveTime::MIN));
        let x0 = view.world_to_screen(start).max(-1.0);
        let x1 = view
            .world_to_screen(start + 1.0)
            .min(f64::from(self.w) + 1.0);
        if x0 < x1 {
            let color = with_alpha(theme::LINE, TODAY_ALPHA);
            p.rect(x0 as f32, 0.0, (x1 - x0) as f32, self.h, 0.0, color);
        }
    }

    /// The background grid at the zoom's step: a thin line at each step, labeled at the top
    /// where the labels don't overlap.
    fn grid(&self, p: &mut Painter) {
        let lines = grid_lines(&self.state.axis, &self.state.view);
        let color = with_alpha(theme::BORDER, GRID_LINE_ALPHA);
        for line in &lines {
            let x = (line.x as f32).round();
            if (0.0..=self.w).contains(&x) {
                p.line(x, 0.0, x, self.h, GRID_LINE_WIDTH, color);
            }
        }
        let texts = grid_texts(
            &lines,
            self.state.view.day_px,
            self.look.lang,
            self.today,
            |s| text_width(GRID_SIZE, s),
            |start| tr_fmt("timeline.grid.week", &[start]),
        );
        for text in texts {
            let color = if text.today {
                theme::TEXT
            } else {
                theme::MUTED_TEXT
            };
            p.text(GRID_SIZE, text.x, GRID_LABEL_TOP, &text.text, color);
        }
    }

    /// Each lane's saved line, straight, from its oldest save (or the left edge, when older
    /// saves are still to load) to its newest. It reaches a station that is growing in as it
    /// grows. The current lane's glows.
    fn line(&self, p: &mut Painter) {
        for (k, lane) in self.data.lanes.iter().enumerate() {
            if lane.saves.is_empty() {
                continue;
            }
            let last = lane.saves.end - 1;
            let mut end = self.x(last);
            if last > lane.saves.start {
                let before = self.x(last - 1);
                end = before + (end - before) * self.grown(last);
            }
            let start = if lane.continues_left {
                -OVERDRAW
            } else {
                self.x(lane.saves.start)
            };
            let (x0, x1) = (start.max(-OVERDRAW), end.min(self.w + OVERDRAW));
            if x0 < x1 {
                let y = self.state.lane_line_y(k) as f32;
                if self.data.current_lane == Some(k) {
                    let glow = with_alpha(theme::LINE, GLOW_ALPHA);
                    p.line(x0, y, x1, y, GLOW_WIDTH, glow);
                }
                p.line(x0, y, x1, y, LINE_WIDTH, theme::LINE);
            }
        }
    }

    /// The dashed amber connector from "You are here" to "Now".
    fn connector(&self, p: &mut Painter) {
        let Some(here) = self.here else {
            return;
        };
        let from = (self.x(here) + HERE_RING_R + 3.0).max(-OVERDRAW);
        let to = (self.now_x - NOW_R - 3.0).min(self.w + OVERDRAW);
        let color = with_alpha(theme::UNSAVED, 0.85 * self.now_alpha());
        // Along "Now"'s line, wherever "You are here" is.
        let y = self.now_y;
        for (x0, x1) in dash_spans(from, to, CONNECTOR_DASH, CONNECTOR_GAP) {
            p.line(x0, y, x1, y, CONNECTOR_WIDTH, color);
        }
    }

    /// Every ball near the view but the pinned ones, which are painted on top of them.
    fn balls(&self, p: &mut Painter) {
        for item in self.balls_to_paint() {
            match *item {
                Item::Single { index, x } => self.station(p, index, x as f32),
                Item::Group {
                    ref members,
                    x,
                    radius,
                } => self.group_ball(p, members, x as f32, radius as f32),
            }
        }
    }

    /// The balls [`Scene::balls`] paints: those that can show (as far past the edges as the
    /// labels reach), but the pinned ones. The layout runs a screen width or more past each
    /// edge, and a group's count costs a measure of its text.
    fn balls_to_paint(&self) -> impl Iterator<Item = &Item> {
        let reach = f64::from(LABEL_REACH);
        let near = -reach..f64::from(self.w) + reach;
        self.items.iter().filter(move |item| {
            let pinned = matches!(**item, Item::Single { index, .. } if self.is_pinned(index));
            !pinned && near.contains(&item.x())
        })
    }

    /// A single save: a ring with a dark center, with the halo when it is selected.
    fn station(&self, p: &mut Painter, index: usize, cx: f32) {
        let (y, r) = (self.y(index), STATION_R * self.grown(index));
        if self.selected == Some(index) {
            halo(p, cx, y, r, theme::LINE);
        } else if self.hover == Some(&Hit::Save(index)) {
            p.circle(cx, y, r + HOVER_EXTRA, with_alpha(theme::TEXT, HOVER_ALPHA));
        }
        p.circle(cx, y, r, theme::BG);
        ring(p, cx, y, r, STATION_STROKE.min(r * 0.36), theme::LINE);
    }

    /// A group: a tinted ball with a cyan ring and the count inside.
    fn group_ball(&self, p: &mut Painter, members: &Range<usize>, cx: f32, r: f32) {
        let y = self.y(members.start);
        if matches!(self.hover, Some(Hit::Group(m)) if m == members) {
            p.circle(cx, y, r + HOVER_EXTRA, with_alpha(theme::TEXT, HOVER_ALPHA));
        }
        p.circle(cx, y, r, theme::BG);
        p.circle(cx, y, r, with_alpha(theme::LINE, GROUP_FILL_ALPHA));
        ring(p, cx, y, r, GROUP_STROKE, theme::LINE);
        let count = members.len().to_string();
        // As large as fits inside the ring.
        let room = 2.0 * r - 2.0 * GROUP_STROKE - 2.0;
        let mut size = COUNT_SIZE;
        while size > 6.0 && text_width(size, &count) > room {
            size -= 1.0;
        }
        let tw = text_width(size, &count);
        p.text(
            size,
            cx - tw / 2.0,
            y - line_height(size) / 2.0,
            &count,
            theme::TEXT,
        );
    }

    /// The selected save, on top of its neighbours, unless it is "You are here".
    fn selected_station(&self, p: &mut Painter) {
        let Some(selected) = self.selected.filter(|&i| Some(i) != self.here) else {
            return;
        };
        if let Some(x) = self.single_x(selected) {
            self.station(p, selected, x);
        }
    }

    /// The latest save: a filled dot, a dark gap and a faint ring, on top of every other ball.
    fn here_station(&self, p: &mut Painter) {
        let Some(here) = self.here_in_view() else {
            return;
        };
        let (cx, y, g) = (self.x(here), self.y(here), self.grown(here));
        if self.selected == Some(here) {
            halo(p, cx, y, HERE_RING_R * g, theme::LINE);
        } else if self.hover == Some(&Hit::Save(here)) {
            let r = (HERE_RING_R + HOVER_EXTRA) * g;
            p.circle(cx, y, r, with_alpha(theme::TEXT, HOVER_ALPHA));
        }
        p.circle(cx, y, HERE_RING_R * g, theme::BG);
        let ring_color = with_alpha(theme::LINE, HERE_RING_ALPHA);
        ring(p, cx, y, HERE_RING_R * g, HERE_RING_STROKE, ring_color);
        p.circle(cx, y, HERE_R * g, theme::LINE);
    }

    /// "Now": a dashed amber circle with the number of unsaved changes inside.
    fn now_station(&self, p: &mut Painter) {
        if !self.now_in_view() {
            return;
        }
        let (cx, y, alpha) = (self.now_x, self.now_y, self.now_alpha());
        let color = with_alpha(theme::UNSAVED, alpha);
        if self.data.selection == Selection::Now {
            halo(p, cx, y, NOW_R, color);
        } else if self.hover == Some(&Hit::Now) {
            p.circle(
                cx,
                y,
                NOW_R + HOVER_EXTRA,
                with_alpha(theme::TEXT, HOVER_ALPHA),
            );
        }
        p.circle(cx, y, NOW_R, theme::BG);
        let step = TAU / NOW_DASHES as f32;
        let dash = step * NOW_DASH_FILL;
        for k in 0..NOW_DASHES {
            // Dashes centered in their steps, the first one at the top.
            let a0 = -FRAC_PI_2 + k as f32 * step + (step - dash) / 2.0;
            let [from, c0, c1, to] = arc_bezier(cx, y, NOW_R, a0, a0 + dash);
            p.curve(from, c0, c1, to, NOW_STROKE, color);
        }
        if self.data.unsaved > 0 {
            let count = self.data.unsaved.to_string();
            let size = match count.len() {
                0..=2 => COUNT_SIZE,
                3 => COUNT_SIZE * 0.8,
                _ => COUNT_SIZE * 0.65,
            };
            let tw = text_width(size, &count);
            let ty = y - line_height(size) / 2.0;
            p.text(size, cx - tw / 2.0, ty, &count, theme::UNSAVED);
        }
    }

    /// Under each ball, a label no wider than its room (see [`label_rooms`]): a save's name and
    /// when it was made, or a group's count and the times it spans. "You are here" and the
    /// selected save get their room first; the labels around them give way. Lane by lane:
    /// labels of different lanes never meet.
    fn labels(&self, p: &mut Painter) {
        let mut start = 0;
        while start < self.items.len() {
            let lane = self.state.lane_of(first_save(&self.items[start]));
            let count = self.items[start..]
                .iter()
                .take_while(|item| self.state.lane_of(first_save(item)) == lane)
                .count();
            self.lane_labels(p, &self.items[start..start + count]);
            start += count;
        }
    }

    /// The labels of `items`, the balls of one lane.
    fn lane_labels(&self, p: &mut Painter, items: &[Item]) {
        let Some(first) = items.first() else {
            return;
        };
        let y = self.item_y(first);
        // "Now"'s label takes room only on its own line.
        let now_x = if y == self.now_y {
            self.now_x
        } else {
            f32::INFINITY
        };
        let mut xs: Vec<f32> = items.iter().map(|item| item.x() as f32).collect();
        let pinned: Vec<bool> = items
            .iter()
            .map(|item| matches!(*item, Item::Single { index, .. } if self.is_pinned(index)))
            .collect();
        let width = |k: usize, room: f32| {
            let (title, detail) = self.label(&items[k], room)?;
            Some(text_width(NAME_SIZE, &title).max(text_width(DETAIL_SIZE, &detail)))
        };
        let rooms = label_rooms(&xs, now_x, &pinned, width);
        xs.push(now_x);
        for (k, item) in items.iter().enumerate() {
            let cx = xs[k];
            if !(-LABEL_REACH..self.w + LABEL_REACH).contains(&cx) {
                continue;
            }
            let Some((title, detail)) = rooms[k].and_then(|room| self.label(item, room)) else {
                continue;
            };
            let y = y + LABEL_TOP;
            centered_text(p, NAME_SIZE, cx, y, &title, theme::TEXT);
            let y = y + line_height(NAME_SIZE);
            centered_text(p, DETAIL_SIZE, cx, y, &detail, theme::MUTED_TEXT);
        }
    }

    /// The label of `item` in `room` pixels: a save's name, cut to fit, and when it was made;
    /// or a group's count (already inside its ball, so whole or not at all) and the times it
    /// spans. The second line shows whole or not at all. `None` when nothing fits.
    fn label(&self, item: &Item, room: f32) -> Option<(String, String)> {
        let name_width = |s: &str| text_width(NAME_SIZE, s);
        let (title, detail) = match item {
            Item::Single { index, .. } => {
                let save = self.data.saves.get(*index)?;
                let name = fit_label(&save.name, LABEL_CHARS, room, name_width);
                (name, self.when(save))
            }
            Item::Group { members, .. } => {
                let count = tr_fmt("timeline.group.count", &[&members.len().to_string()]);
                if name_width(&count) > room {
                    return None;
                }
                let span = match self.step {
                    GridStep::Weeks | GridStep::Months => String::new(),
                    _ => self.time_span(members),
                };
                (count, span)
            }
        };
        if title.is_empty() {
            return None;
        }
        let detail = if text_width(DETAIL_SIZE, &detail) <= room {
            detail
        } else {
            String::new()
        };
        Some((title, detail))
    }

    /// "Now", and what it holds, under the "Now" station. It may spread right, where nothing
    /// else is, but not left into the label of "You are here".
    fn now_label(&self, p: &mut Painter) {
        if !self.now_in_view() {
            return;
        }
        let cx = self.now_x;
        let left_room = match self.here.filter(|&here| self.y(here) == self.now_y) {
            None => NOW_LABEL_HALF,
            Some(here) => ((cx - self.x(here)) / 2.0 - LABEL_GAP).max(0.0),
        };
        let left = cx - left_room;
        let right = (self.w - EDGE_PAD).max(cx + left_room);
        let max_w = right - left;
        let alpha = self.now_alpha();
        let mut y = self.now_y + LABEL_TOP;

        let title = fit_label(&tr("timeline.now"), usize::MAX, max_w, |s| {
            text_width(NAME_SIZE, s)
        });
        let color = with_alpha(theme::UNSAVED, alpha);
        span_text(p, NAME_SIZE, cx, left, right, y, &title, color);
        y += line_height(NAME_SIZE);
        let detail_width = |s: &str| text_width(DETAIL_SIZE, s);
        let lines = if self.data.unsaved == 0 {
            wrap_lines(
                &tr("timeline.nothing_new"),
                max_w.min(NOW_DETAIL_W),
                2,
                detail_width,
            )
        } else {
            let text = tr_plural("changes.unsaved", self.data.unsaved as i64);
            vec![fit_label(&text, usize::MAX, max_w, detail_width)]
        };
        for line in lines {
            span_text(p, DETAIL_SIZE, cx, left, right, y, &line, theme::MUTED_TEXT);
            y += line_height(DETAIL_SIZE);
        }
    }

    /// "YOU ARE HERE" above the latest save.
    fn here_label(&self, p: &mut Painter) {
        let Some(here) = self.here_in_view() else {
            return;
        };
        let text = tr("timeline.you_are_here").to_uppercase();
        let y = self.y(here) - HERE_RING_R - 6.0 - line_height(TAG_SIZE);
        centered_text(p, TAG_SIZE, self.x(here), y, &text, theme::LINE);
    }

    /// The width and height of the temporary zoom's pill, while it shows.
    fn pill_size(&self) -> Option<(f32, f32)> {
        self.state.temp.active().then(|| {
            let tw = text_width(PILL_SIZE, &tr("timeline.temporary_zoom"));
            (
                tw + 2.0 * PILL_PAD_X,
                line_height(PILL_SIZE) + 2.0 * PILL_PAD_Y,
            )
        })
    }

    /// "Zoomed in · Esc to go back", at the bottom right, while a temporary zoom lasts.
    fn pill(&self, p: &mut Painter) {
        let Some((pw, ph)) = self.pill_size() else {
            return;
        };
        let text = tr("timeline.temporary_zoom");
        let (x, y) = (self.w - 2.0 * EDGE_PAD - pw, self.h - 2.0 * EDGE_PAD - ph);
        p.rect(x, y, pw, ph, ph / 2.0, theme::SURFACE);
        p.rect_outline(x, y, pw, ph, ph / 2.0, 1.0, theme::BORDER);
        p.text(
            PILL_SIZE,
            x + PILL_PAD_X,
            y + PILL_PAD_Y,
            &text,
            theme::TEXT,
        );
    }

    /// What is under the pointer, next to the pointer: a save's full name, date and time and
    /// file count; what a group holds and what a click on it does; what "Now" holds.
    fn tooltip(&self, p: &mut Painter) {
        if !self.look.tooltip || self.look.list.is_some() {
            return;
        }
        let (Some(hover), Some((px, py))) = (self.hover, self.look.pointer) else {
            return;
        };
        let lines = match hover {
            Hit::Save(i) => match self.data.saves.get(*i) {
                Some(save) => self.save_tooltip_lines(save),
                None => return,
            },
            Hit::Group(members) => match self.group_tooltip_lines(members) {
                Some(lines) => lines,
                None => return,
            },
            Hit::Now => self.now_tooltip_lines(),
            // The lanes' names are not drawn yet.
            Hit::LaneName(_) => return,
        };
        let text_w = lines
            .iter()
            .map(|(text, size, _)| text_width(*size, text))
            .fold(0.0, f32::max);
        let text_h: f32 = lines.iter().map(|(_, size, _)| line_height(*size)).sum();
        let (bw, bh) = (text_w + 2.0 * TOOLTIP_PAD_X, text_h + 2.0 * TOOLTIP_PAD_Y);
        // Below and right of the pointer, as Raven places its own; above it when there is no
        // room below. Always inside the widget.
        let x = (px + 12.0).min(self.w - bw - EDGE_PAD).max(EDGE_PAD);
        let mut y = py + 20.0;
        if y + bh > self.h - EDGE_PAD {
            y = (py - bh - 6.0).max(EDGE_PAD);
        }
        p.rect(x, y, bw, bh, TOOLTIP_RADIUS, theme::SURFACE);
        p.rect_outline(x, y, bw, bh, TOOLTIP_RADIUS, 1.0, theme::BORDER);
        let mut ty = y + TOOLTIP_PAD_Y;
        for (text, size, color) in &lines {
            p.text(*size, x + TOOLTIP_PAD_X, ty, text, *color);
            ty += line_height(*size);
        }
    }

    /// The list of a group's saves, newest first: "name · time" per row.
    fn group_list(&self, p: &mut Painter) {
        let Some(list) = self.look.list else {
            return;
        };
        let frame = list.frame(self.w, self.h, self.y(list.members.start));
        p.rect(
            frame.x,
            frame.y,
            frame.w,
            frame.h,
            LIST_RADIUS,
            theme::SURFACE,
        );
        p.rect_outline(
            frame.x,
            frame.y,
            frame.w,
            frame.h,
            LIST_RADIUS,
            1.0,
            theme::BORDER,
        );
        let measure = |s: &str| text_width(NAME_SIZE, s);
        let text_x = frame.x + LIST_TEXT_PAD;
        let text_w = frame.w - 2.0 * LIST_TEXT_PAD;
        for shown in 0..frame.shown {
            let row = list.scroll + shown;
            let Some(save) = list.save_at(row).and_then(|i| self.data.saves.get(i)) else {
                break;
            };
            let top = frame.y + LIST_PAD + shown as f32 * LIST_ROW_H;
            if list.hover == Some(row) {
                p.rect(
                    frame.x + 3.0,
                    top,
                    frame.w - 6.0,
                    LIST_ROW_H,
                    4.0,
                    theme::HOVERED,
                );
            }
            let time = time_label(self.state.local(save.committed).time());
            let time = format!("{SEPARATOR}{time}");
            let name = fit_label(&save.name, usize::MAX, text_w - measure(&time), measure);
            let selected = list.save_at(row).is_some_and(|i| self.selected == Some(i));
            let color = if selected { theme::LINE } else { theme::TEXT };
            let ty = top + (LIST_ROW_H - line_height(NAME_SIZE)) / 2.0;
            p.text(NAME_SIZE, text_x, ty, &name, color);
            p.text(
                NAME_SIZE,
                text_x + measure(&name),
                ty,
                &time,
                theme::MUTED_TEXT,
            );
        }
        // Where the rows shown are among all of them, when they don't all fit.
        let count = list.len();
        if frame.shown < count {
            let track = frame.h - 2.0 * LIST_PAD;
            let thumb = (track * frame.shown as f32 / count as f32).max(12.0);
            let at = (track - thumb) * list.scroll as f32 / (count - frame.shown) as f32;
            let x = frame.x + frame.w - LIST_SCROLLBAR_W - 3.0;
            let color = with_alpha(theme::MUTED_TEXT, 0.6);
            let r = LIST_SCROLLBAR_W / 2.0;
            p.rect(
                x,
                frame.y + LIST_PAD + at,
                LIST_SCROLLBAR_W,
                thumb,
                r,
                color,
            );
        }
    }
}

/// How wide the label under each ball (at screen xs `xs`, in order, with "Agora" at `now_x`
/// after them) may be, or `None` where it is hidden. Labels are centered under their balls and
/// never overlap, with [`LABEL_GAP`] around each; one with less room than [`MIN_LABEL_W`] is
/// hidden.
///
/// The `pinned` balls' labels ("You are here", the selected save) get their room first, giving
/// way only to each other and to "Agora", whose label takes half the room between it and the
/// ball before it. `width` says how wide such a label really is in a given room (`None`: it
/// shows nothing). Then every other label gets the room left between its neighbours' balls and
/// the pinned labels. When two pinned balls are too close for both labels, the first keeps its
/// own.
fn label_rooms(
    xs: &[f32],
    now_x: f32,
    pinned: &[bool],
    width: impl Fn(usize, f32) -> Option<f32>,
) -> Vec<Option<f32>> {
    let count = xs.len();
    let fits = |room: f32| (room >= MIN_LABEL_W).then_some(room);
    // The room left beside the labels already placed: none for a ball under one of them.
    let clear_of = |taken: &[(f32, f32)], cx: f32, room: f32| {
        taken.iter().fold(room, |room, &(left, right)| {
            let half = if cx < left {
                left - cx
            } else if cx > right {
                cx - right
            } else {
                0.0
            };
            room.min(2.0 * half)
        })
    };
    let mut rooms = vec![None; count];
    let mut taken: Vec<(f32, f32)> = Vec::new();
    let pinned_ks: Vec<usize> = (0..count).filter(|&k| pinned[k]).collect();
    for &k in &pinned_ks {
        let cx = xs[k];
        let others = pinned_ks
            .iter()
            .filter(|&&j| j != k)
            .map(|&j| (xs[j] - cx).abs());
        let nearest = others.fold((now_x - cx).abs(), f32::min);
        let mut room = nearest - 2.0 * LABEL_GAP;
        if room < MIN_LABEL_W && taken.is_empty() {
            room = (now_x - cx).abs() - 2.0 * LABEL_GAP;
        }
        rooms[k] = fits(clear_of(&taken, cx, room));
        if let Some(w) = rooms[k].and_then(|room| width(k, room)) {
            taken.push((cx - w / 2.0 - LABEL_GAP, cx + w / 2.0 + LABEL_GAP));
        }
    }
    for k in (0..count).filter(|&k| !pinned[k]) {
        let cx = xs[k];
        // The nearest balls that aren't pinned (or "Agora"): pinned ones are cleared above.
        let before = (0..k)
            .rev()
            .find(|&j| !pinned[j])
            .map_or(f32::INFINITY, |j| cx - xs[j]);
        let next = (k + 1..count)
            .find(|&j| !pinned[j])
            .map_or(now_x, |j| xs[j]);
        let room = before.min(next - cx) - 2.0 * LABEL_GAP;
        rooms[k] = fits(clear_of(&taken, cx, room));
    }
    rooms
}

/// A label of the background grid: its text, where it starts, and whether it names today.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct GridText {
    pub x: f32,
    pub text: String,
    pub today: bool,
}

/// The labels of the grid `lines` at `day_px`, as wide as `measure` says, in increasing x. Each
/// starts a little right of its line; one that would start off the left edge, or too close to
/// the label before it, is skipped. The first label shown names its day, month or week in
/// full ("sex 31 · 13h", "dom 26 out", "semana de 28 set" made by `week`), and so does every
/// midnight among hours and minutes. Months are labeled every [`month_label_stride`] months.
pub(super) fn grid_texts(
    lines: &[GridLine],
    day_px: f64,
    lang: Lang,
    today: NaiveDate,
    measure: impl Fn(&str) -> f32,
    week: impl Fn(&str) -> String,
) -> Vec<GridText> {
    let stride = month_label_stride(day_px);
    let mut texts: Vec<GridText> = Vec::new();
    let mut right = f32::NEG_INFINITY;
    for line in lines {
        let x = line.x as f32 + GRID_LABEL_PAD;
        let skipped_month = matches!(line.label, GridLabel::Month { month, .. } if month.saturating_sub(1) % stride != 0);
        let on_screen = x >= 0.0 && x.is_finite();
        if !on_screen || skipped_month || x < right + GRID_LABEL_GAP {
            continue;
        }
        let first = texts.is_empty();
        let with_day = |date: NaiveDate, first_of_day: bool, tick: String| {
            if first || first_of_day {
                format!("{}{SEPARATOR}{tick}", day_label(lang, date, today))
            } else {
                tick
            }
        };
        let (text, is_today) = match line.label {
            GridLabel::Minute {
                date,
                hour,
                minute,
                first_of_day,
            } => (
                with_day(date, first_of_day, minute_label(hour, minute)),
                false,
            ),
            GridLabel::Hour {
                date,
                hour,
                first_of_day,
            } => (with_day(date, first_of_day, hour_label(lang, hour)), false),
            GridLabel::Day(date) => (day_label_full(lang, date, today, first), date == today),
            GridLabel::Week(monday) => {
                let start = week_start_label(lang, monday);
                (if first { week(&start) } else { start }, false)
            }
            GridLabel::Month { year, month } => (month_label(lang, year, month), false),
        };
        right = x + measure(&text);
        texts.push(GridText {
            x,
            text,
            today: is_today,
        });
    }
    texts
}

/// Label every how many months the grid's month lines, so that labels are at least
/// [`GRID_MIN_SPACING_PX`] apart at `day_px`: every month, or every 2nd, 3rd, 6th or 12th.
pub(super) fn month_label_stride(day_px: f64) -> u32 {
    let month_px = DAYS_PER_MONTH * day_px;
    MONTH_STRIDES
        .into_iter()
        .find(|&stride| f64::from(stride) * month_px >= GRID_MIN_SPACING_PX)
        .unwrap_or(12)
}

/// The first line of a group's tooltip: its translation key, and what goes in its `{0}`,
/// `{1}`...
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GroupTitle {
    pub key: &'static str,
    pub args: Vec<String>,
}

/// How a group of `count` saves made from the local day `first` to `last` is named, by how far
/// apart they are: one day ("15 saves em 31 de out."), one week from Monday to Sunday ("9 saves
/// na semana de 26 out"), one month ("22 saves em outubro"), or a range of dates.
pub(super) fn group_title(
    lang: Lang,
    count: usize,
    first: NaiveDate,
    last: NaiveDate,
) -> GroupTitle {
    let n = count.to_string();
    let monday = |date: NaiveDate| {
        let back = TimeDelta::days(i64::from(date.weekday().num_days_from_monday()));
        date.checked_sub_signed(back).unwrap_or(date)
    };
    let (key, args) = if first == last {
        (
            tr_key("timeline.group.day"),
            vec![n, short_date(lang, first)],
        )
    } else if monday(first) == monday(last) {
        let start = week_start_label(lang, monday(first));
        (tr_key("timeline.group.week"), vec![n, start])
    } else if (first.year(), first.month()) == (last.year(), last.month()) {
        let month = month_name(lang, first.month());
        (tr_key("timeline.group.month"), vec![n, month])
    } else {
        let range = vec![n, short_date(lang, first), short_date(lang, last)];
        (tr_key("timeline.group.range"), range)
    };
    GroupTitle { key, args }
}

/// The second line of a group's tooltip: what a click on it does. It zooms in when that `splits`
/// the group, and lists its saves otherwise.
pub(super) fn group_hint(splits: bool) -> &'static str {
    if splits {
        tr_key("timeline.group.hint_zoom")
    } else {
        tr_key("timeline.group.hint_list")
    }
}

/// The list of the saves of a group that can't split any further, open under its ball. Rows
/// count from the newest save.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct GroupList {
    pub members: Range<usize>,
    /// The screen x and radius of the group's ball (the view does not move while the list is
    /// open).
    pub x: f32,
    pub radius: f32,
    /// How many rows are scrolled past.
    pub scroll: usize,
    /// The part of a line the wheel turned that has not scrolled a row yet.
    wheel: f32,
    /// The row under the pointer.
    pub hover: Option<usize>,
}

/// Where a group's list is, and how many of its rows show at once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ListBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub shown: usize,
}

impl ListBox {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        (self.x..self.x + self.w).contains(&x) && (self.y..self.y + self.h).contains(&y)
    }
}

impl GroupList {
    pub fn new(members: Range<usize>, x: f32, radius: f32) -> GroupList {
        GroupList {
            members,
            x,
            radius,
            scroll: 0,
            wheel: 0.0,
            hover: None,
        }
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// The index of the save on `row`, newest first.
    pub fn save_at(&self, row: usize) -> Option<usize> {
        (row < self.len()).then(|| self.members.end - 1 - row)
    }

    /// Where the list goes in a widget `w` by `h` whose line is at `line_y`: under the ball,
    /// centered on it and inside the widget, as tall as its rows or as many of them as fit.
    pub fn frame(&self, w: f32, h: f32, line_y: f32) -> ListBox {
        let width = LIST_W.min(w - 2.0 * EDGE_PAD).max(0.0);
        let x = (self.x - width / 2.0)
            .min(w - EDGE_PAD - width)
            .max(EDGE_PAD);
        let fitting = |top: f32| {
            let rows = ((h - EDGE_PAD - top - 2.0 * LIST_PAD) / LIST_ROW_H).floor();
            if rows > 0.0 { rows as usize } else { 0 }
        };
        let count = self.len();
        let below = line_y + self.radius + LIST_GAP;
        let (y, shown) = if fitting(below) >= count.min(LIST_MIN_ROWS) {
            (below, count.min(fitting(below)))
        } else {
            let shown = count.min(fitting(EDGE_PAD)).max(1);
            let height = shown as f32 * LIST_ROW_H + 2.0 * LIST_PAD;
            ((h - EDGE_PAD - height).max(EDGE_PAD), shown)
        };
        ListBox {
            x,
            y,
            w: width,
            h: shown as f32 * LIST_ROW_H + 2.0 * LIST_PAD,
            shown,
        }
    }

    /// The row at (`x`, `y`) in the list at `frame`.
    pub fn row_at(&self, frame: &ListBox, x: f32, y: f32) -> Option<usize> {
        if !frame.contains(x, y) {
            return None;
        }
        let shown = ((y - frame.y - LIST_PAD) / LIST_ROW_H).floor();
        if !(0.0..frame.shown as f32).contains(&shown) {
            return None;
        }
        let row = self.scroll + shown as usize;
        (row < self.len()).then_some(row)
    }

    /// Scrolls by a wheel turn of `lines`: up (positive) toward the newest save. Turns of less
    /// than a line (a touchpad's) add up until they make one.
    pub fn scroll_by(&mut self, lines: f32, frame: &ListBox) {
        if !lines.is_finite() {
            return;
        }
        let most = self.len().saturating_sub(frame.shown);
        self.wheel += lines;
        // A little slack, so that ten tenths of a line make one despite rounding.
        let whole = (self.wheel + WHEEL_SLACK.copysign(self.wheel)).trunc();
        self.wheel -= whole;
        let next = self.scroll as f32 - whole;
        self.scroll = if next > 0.0 {
            (next as usize).min(most)
        } else {
            0
        };
    }
}

/// The width of each lane's name at [`LANE_NAME_SIZE`], in lane order: their name boxes.
pub(super) fn lane_label_widths(data: &TimelineData) -> Vec<f64> {
    data.lanes
        .iter()
        .map(|lane| f64::from(text_width(LANE_NAME_SIZE, &lane.label)))
        .collect()
}

/// The first save `item` holds.
fn first_save(item: &Item) -> usize {
    match item {
        Item::Single { index, .. } => *index,
        Item::Group { members, .. } => members.start,
    }
}

fn with_alpha(c: Rgba, alpha: f32) -> Rgba {
    Rgba(c.0, c.1, c.2, c.3 * alpha)
}

fn line_height(size: f32) -> f32 {
    size * LINE_HEIGHT
}

/// A circle outline whose stroke is centered on radius `r`.
fn ring(p: &mut Painter, cx: f32, cy: f32, r: f32, thickness: f32, c: Rgba) {
    let outer = r + thickness / 2.0;
    let d = 2.0 * outer;
    p.rect_outline(cx - outer, cy - outer, d, d, outer, thickness, c);
}

/// The selected station's halo: a soft glow and a thin ring around a station of radius `r`.
fn halo(p: &mut Painter, cx: f32, cy: f32, r: f32, c: Rgba) {
    p.circle(cx, cy, r + HALO_EXTRA, with_alpha(c, HALO_ALPHA));
    ring(
        p,
        cx,
        cy,
        r + HALO_RING_EXTRA,
        1.5,
        with_alpha(c, HALO_RING_ALPHA),
    );
}

fn centered_text(p: &mut Painter, size: f32, cx: f32, y: f32, text: &str, c: Rgba) {
    if !text.is_empty() {
        p.text(size, cx - text_width(size, text) / 2.0, y, text, c);
    }
}

/// Text centered on `cx`, moved as little as needed to stay between `left` and `right`.
#[allow(clippy::too_many_arguments)]
fn span_text(
    p: &mut Painter,
    size: f32,
    cx: f32,
    left: f32,
    right: f32,
    y: f32,
    text: &str,
    c: Rgba,
) {
    if text.is_empty() {
        return;
    }
    let tw = text_width(size, text);
    let x = (cx - tw / 2.0).min(right - tw).max(left);
    p.text(size, x, y, text, c);
}

/// Eases out (cubic): fast at first, settling at 1. `t` is clamped to 0..1.
pub(super) fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// The dashes of a dashed line from `from` to `to`: `dash` long, `gap` apart, the last one cut
/// at `to`.
fn dash_spans(from: f32, to: f32, dash: f32, gap: f32) -> Vec<(f32, f32)> {
    let mut spans = Vec::new();
    if !(dash > 0.0 && gap >= 0.0 && from.is_finite() && to.is_finite()) {
        return spans;
    }
    let mut x = from;
    while x < to {
        spans.push((x, (x + dash).min(to)));
        x += dash + gap;
    }
    spans
}

/// The cubic Bézier (start, two control points, end) closest to the arc of the circle around
/// (`cx`, `cy`) with radius `r`, from angle `a0` to `a1` in radians (clockwise on screen, where
/// y grows down). Close enough for arcs up to a quarter turn.
fn arc_bezier(cx: f32, cy: f32, r: f32, a0: f32, a1: f32) -> [(f32, f32); 4] {
    let k = 4.0 / 3.0 * ((a1 - a0) / 4.0).tan() * r;
    let (s0, c0) = a0.sin_cos();
    let (s1, c1) = a1.sin_cos();
    let start = (cx + r * c0, cy + r * s0);
    let end = (cx + r * c1, cy + r * s1);
    [
        start,
        (start.0 - k * s0, start.1 + k * c0),
        (end.0 + k * s1, end.1 - k * c1),
        end,
    ]
}

/// `text` cut to at most `max_chars` characters, then further until `measure` says it is no
/// wider than `max_w`, ending in `…` when cut. Empty when not even `…` fits.
fn fit_label(text: &str, max_chars: usize, max_w: f32, measure: impl Fn(&str) -> f32) -> String {
    let first = truncate_label(text, max_chars);
    if measure(&first) <= max_w {
        return first;
    }
    // The longest cut that fits: zero characters always does, `first`'s count does not.
    let (mut fits, mut too_wide) = (0, first.chars().count());
    while too_wide - fits > 1 {
        let mid = (fits + too_wide) / 2;
        if measure(&truncate_label(text, mid)) <= max_w {
            fits = mid;
        } else {
            too_wide = mid;
        }
    }
    truncate_label(text, fits)
}

/// `text` broken at spaces into at most `max_lines` lines no wider than `max_w` by `measure`.
/// A word wider than a line is cut, and what does not fit in the last line ends it with `…`.
fn wrap_lines(
    text: &str,
    max_w: f32,
    max_lines: usize,
    measure: impl Fn(&str) -> f32,
) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut lines = Vec::new();
    let mut next = 0;
    while next < words.len() && lines.len() < max_lines {
        if lines.len() + 1 == max_lines {
            let rest = words[next..].join(" ");
            lines.push(fit_label(&rest, usize::MAX, max_w, &measure));
            break;
        }
        let mut line = words[next].to_owned();
        next += 1;
        while let Some(word) = words.get(next) {
            let longer = format!("{line} {word}");
            if measure(&longer) > max_w {
                break;
            }
            line = longer;
            next += 1;
        }
        lines.push(fit_label(&line, usize::MAX, max_w, &measure));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::super::axis::MIN_DAY_PX;
    use super::*;
    use chrono::{DateTime, FixedOffset};

    /// One unit of width per character.
    fn chars(s: &str) -> f32 {
        s.chars().count() as f32
    }

    fn assert_close(a: f32, b: f32, tolerance: f32) {
        assert!(
            (a - b).abs() <= tolerance,
            "{a} is not within {tolerance} of {b}"
        );
    }

    #[test]
    fn fit_label_caps_the_characters_then_the_width() {
        let name = "Carrinho de compras funcionando, falta só calcular o frete";
        assert_eq!(
            fit_label(name, 24, 100.0, chars),
            "Carrinho de compras fun…"
        );
        assert_eq!(fit_label(name, 24, 10.0, chars), "Carrinho…");
        assert_eq!(fit_label("Modo escuro", 24, 100.0, chars), "Modo escuro");
        assert_eq!(fit_label("Modo escuro", 24, 11.0, chars), "Modo escuro");
        assert_eq!(fit_label("Modo escuro", 24, 1.0, chars), "…");
        assert_eq!(fit_label("Modo escuro", 24, 0.5, chars), "");
        // Counted in characters, never splitting one.
        assert_eq!(fit_label("ação útil", usize::MAX, 5.0, chars), "ação…");
    }

    #[test]
    fn wrap_lines_breaks_at_spaces_and_ends_the_last_line_with_an_ellipsis() {
        let text = "Nada novo desde o último save";
        assert_eq!(
            wrap_lines(text, 17.0, 2, chars),
            ["Nada novo desde o", "último save"]
        );
        assert_eq!(wrap_lines(text, 100.0, 2, chars), [text]);
        assert_eq!(
            wrap_lines(text, 10.0, 2, chars),
            ["Nada novo", "desde o ú…"]
        );
        // A word wider than a line is cut.
        assert_eq!(
            wrap_lines("Supercalifragilístico", 6.0, 3, chars),
            ["Super…"]
        );
        assert!(wrap_lines(text, 10.0, 0, chars).is_empty());
        assert!(wrap_lines("   ", 10.0, 2, chars).is_empty());
    }

    #[test]
    fn dashes_are_cut_at_the_end() {
        assert_eq!(
            dash_spans(0.0, 20.0, 4.0, 4.0),
            [(0.0, 4.0), (8.0, 12.0), (16.0, 20.0)]
        );
        assert_eq!(dash_spans(0.0, 10.0, 4.0, 4.0), [(0.0, 4.0), (8.0, 10.0)]);
        assert!(dash_spans(5.0, 5.0, 4.0, 4.0).is_empty());
        assert!(dash_spans(9.0, 2.0, 4.0, 4.0).is_empty());
        assert!(dash_spans(0.0, 10.0, 0.0, 4.0).is_empty());
        assert!(dash_spans(f32::NEG_INFINITY, 10.0, 4.0, 4.0).is_empty());
    }

    #[test]
    fn arc_bezier_stays_on_the_circle() {
        let (cx, cy, r) = (100.0, 50.0, 11.0);
        let step = TAU / NOW_DASHES as f32;
        for k in 0..NOW_DASHES {
            let a0 = -FRAC_PI_2 + k as f32 * step;
            let [p0, p1, p2, p3] = arc_bezier(cx, cy, r, a0, a0 + step * NOW_DASH_FILL);
            let distance = |(x, y): (f32, f32)| (x - cx).hypot(y - cy);
            assert_close(distance(p0), r, 1e-4);
            assert_close(distance(p3), r, 1e-4);
            // The curve's middle point.
            let mid = (
                (p0.0 + 3.0 * p1.0 + 3.0 * p2.0 + p3.0) / 8.0,
                (p0.1 + 3.0 * p1.1 + 3.0 * p2.1 + p3.1) / 8.0,
            );
            assert_close(distance(mid), r, 1e-3);
        }
        // Angles run clockwise on screen: a quarter turn from the top ends on the right.
        let [start, .., end] = arc_bezier(0.0, 0.0, 10.0, -FRAC_PI_2, 0.0);
        assert_close(start.0, 0.0, 1e-4);
        assert_close(start.1, -10.0, 1e-4);
        assert_close(end.0, 10.0, 1e-4);
        assert_close(end.1, 0.0, 1e-4);
    }

    #[test]
    fn ease_out_starts_fast_and_settles() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert_close(ease_out(0.5), 0.875, 1e-6);
        assert_eq!(ease_out(-1.0), 0.0);
        assert_eq!(ease_out(7.0), 1.0);
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|&a| a.to_owned()).collect()
    }

    #[test]
    fn group_tooltip_text_by_span() {
        let title = |n, first, last| group_title(Lang::PtBr, n, first, last);
        // All on Saturday 31 October 2026.
        assert_eq!(
            title(15, date(2026, 10, 31), date(2026, 10, 31)),
            GroupTitle {
                key: "timeline.group.day",
                args: strings(&["15", "31 de out."])
            }
        );
        // Monday 26 October to Sunday 1 November: one week, across two months.
        assert_eq!(
            title(9, date(2026, 10, 26), date(2026, 11, 1)),
            GroupTitle {
                key: "timeline.group.week",
                args: strings(&["9", "26 out"])
            }
        );
        // Two weeks of October.
        assert_eq!(
            title(22, date(2026, 10, 5), date(2026, 10, 28)),
            GroupTitle {
                key: "timeline.group.month",
                args: strings(&["22", "outubro"])
            }
        );
        // From September to October.
        assert_eq!(
            title(30, date(2026, 9, 28), date(2026, 10, 12)),
            GroupTitle {
                key: "timeline.group.range",
                args: strings(&["30", "28 de set.", "12 de out."])
            }
        );
        // The same month of two years is a range; dates follow the language.
        assert_eq!(
            group_title(Lang::En, 4, date(2025, 10, 5), date(2026, 10, 6)),
            GroupTitle {
                key: "timeline.group.range",
                args: strings(&["4", "Oct 5", "Oct 6"])
            }
        );
        assert_eq!(
            group_title(Lang::En, 3, date(2026, 10, 5), date(2026, 10, 28)).args,
            strings(&["3", "October"])
        );
        // The second line: zoom in, or the list when zooming in would not split it.
        assert_eq!(group_hint(true), "timeline.group.hint_zoom");
        assert_eq!(group_hint(false), "timeline.group.hint_list");
    }

    #[test]
    fn month_labels_keep_apart() {
        // At the deepest zoom out a month is about 36 px: every other month is labeled.
        assert_eq!(month_label_stride(MIN_DAY_PX), 2);
        // About 61 px a month: every month.
        assert_eq!(month_label_stride(2.0), 1);
        assert_eq!(month_label_stride(8.0), 1);
        // Narrower still: a quarter, half a year, a year.
        assert_eq!(month_label_stride(0.7), 3);
        assert_eq!(month_label_stride(0.4), 6);
        assert_eq!(month_label_stride(0.1), 12);
        assert_eq!(month_label_stride(0.0), 12);
    }

    fn line(x: f64, label: GridLabel) -> GridLine {
        GridLine { x, label }
    }

    /// The grid texts of `lines` at `day_px`, measured one unit per character, today being
    /// Monday 2 November 2026.
    fn texts(lines: &[GridLine], day_px: f64) -> Vec<(f32, String, bool)> {
        let week = |start: &str| format!("semana de {start}");
        grid_texts(lines, day_px, Lang::PtBr, date(2026, 11, 2), chars, week)
            .into_iter()
            .map(|t| (t.x, t.text, t.today))
            .collect()
    }

    fn label(x: f32, text: &str) -> (f32, String, bool) {
        (x, text.to_owned(), false)
    }

    #[test]
    fn grid_labels_name_the_first_and_skip_overlaps() {
        let pad = GRID_LABEL_PAD;
        // Days: a line off the left edge has no label; the first label in view names its
        // month, and today says so.
        let days: Vec<GridLine> = [(-140.0, 30), (0.0, 31), (140.0, 1), (280.0, 2), (420.0, 3)]
            .into_iter()
            .map(|(x, d)| {
                let month = if d > 3 { 10 } else { 11 };
                line(x, GridLabel::Day(date(2026, month, d)))
            })
            .collect();
        assert_eq!(
            texts(&days, 140.0),
            [
                label(pad, "sáb 31 out"),
                label(140.0 + pad, "dom 1 nov"),
                (280.0 + pad, "hoje".to_owned(), true),
                label(420.0 + pad, "ter 3"),
            ]
        );

        // Hours: the first one names its day, and so does midnight. A label that would
        // overlap the one before it is skipped.
        let hour = |x: f64, d: u32, hour: u32| {
            line(
                x,
                GridLabel::Hour {
                    date: date(2026, 10, d),
                    hour,
                    first_of_day: hour == 0,
                },
            )
        };
        let hours = [
            hour(10.0, 30, 22),
            hour(25.0, 30, 23),
            hour(40.0, 31, 0),
            hour(55.0, 31, 1),
            hour(70.0, 31, 2),
        ];
        assert_eq!(
            texts(&hours, 1_440.0),
            [
                label(10.0 + pad, "sex 30 · 22h"),
                label(40.0 + pad, "sáb 31 · 0h"),
                label(70.0 + pad, "2h"),
            ]
        );

        // Weeks: "semana de" on the first, the date alone after it.
        let weeks = [
            line(20.0, GridLabel::Week(date(2026, 9, 28))),
            line(230.0, GridLabel::Week(date(2026, 10, 5))),
        ];
        assert_eq!(
            texts(&weeks, 30.0),
            [
                label(20.0 + pad, "semana de 28 set"),
                label(230.0 + pad, "5 out"),
            ]
        );

        // Months at the deepest zoom out: every line, but a label every other month (from
        // January).
        let months: Vec<GridLine> = (0..4)
            .map(|i| {
                line(
                    5.0 + 36.0 * f64::from(i),
                    GridLabel::Month {
                        year: 2026,
                        month: 9 + i,
                    },
                )
            })
            .collect();
        assert_eq!(
            texts(&months, MIN_DAY_PX),
            [label(5.0 + pad, "set 2026"), label(77.0 + pad, "nov 2026")]
        );
    }

    #[test]
    fn pinned_labels_get_room_first() {
        // Labels at most 40 wide.
        let width = |_: usize, room: f32| Some(room.min(40.0));
        // "You are here" (124) right after two saves; "Agora" at 180. Its label takes 40 of the
        // 48 it may, and the labels under it give way. The first save keeps the room to the
        // second one's ball.
        let rooms = label_rooms(
            &[0.0, 100.0, 110.0, 124.0],
            180.0,
            &[false, false, false, true],
            width,
        );
        assert_eq!(rooms, [Some(92.0), None, None, Some(48.0)]);
        // Unpinned, it would give way instead (wave 1's rule: the room to the nearest ball).
        let rooms = label_rooms(&[0.0, 100.0, 110.0, 160.0], 216.0, &[false; 4], width);
        assert_eq!(rooms, [Some(92.0), None, None, Some(42.0)]);
        // Two pinned balls share the room between them...
        let rooms = label_rooms(&[50.0, 120.0], 200.0, &[true, true], width);
        assert_eq!(rooms, [Some(62.0), Some(62.0)]);
        // ...and when they are too close for both, the first keeps its label.
        let rooms = label_rooms(&[50.0, 50.5], 200.0, &[true, true], width);
        assert_eq!(rooms, [Some(142.0), None]);
        // A pinned label that shows nothing takes no room: the labels on either side of it
        // keep apart from each other.
        let shows_nothing = |_: usize, _: f32| None;
        let rooms = label_rooms(
            &[100.0, 110.0, 150.0],
            200.0,
            &[false, true, false],
            shows_nothing,
        );
        assert_eq!(rooms, [Some(42.0), Some(82.0), Some(42.0)]);
        // With its label, they give way to it.
        let rooms = label_rooms(&[100.0, 110.0, 150.0], 200.0, &[false, true, false], width);
        assert_eq!(rooms, [None, Some(82.0), Some(32.0)]);
        assert!(label_rooms(&[], 180.0, &[], width).is_empty());
    }

    /// "Save {n}", committed at `committed` (RFC 3339).
    fn save_committed(n: usize, committed: &str) -> SaveState {
        save_made(n, DateTime::parse_from_rfc3339(committed).expect("a date"))
    }

    /// "Save {n}", committed at `committed`.
    fn save_made(n: usize, committed: DateTime<FixedOffset>) -> SaveState {
        SaveState {
            id: savestates_core::SaveId(format!("{n:040x}")),
            name: format!("Save {n}"),
            time: committed,
            committed,
            files_changed: 1,
        }
    }

    /// A state 1,000 px wide with `saves`, placed by each time's own offset, and the clock at
    /// `now` (local).
    fn state_with(saves: Vec<SaveState>, now: &str) -> TimelineState {
        let data = TimelineData::one_lane(saves, false, 0);
        let now = now.parse().expect("a local time");
        let own_offset = |t: DateTime<FixedOffset>| t.naive_local();
        TimelineState::new(data, 1000.0, 100.0, own_offset, now)
    }

    fn look(lang: Lang) -> Look<'static> {
        Look {
            lang,
            hover: None,
            pointer: None,
            tooltip: false,
            grow: None,
            list: None,
        }
    }

    #[test]
    fn a_save_tooltip_tells_the_time_its_label_does() {
        // Saved on a summer-time offset: the label and the tooltip both use the local time
        // the save is placed at, not today's offset.
        let saves = vec![
            save_committed(0, "2025-12-30T10:00:00-02:00"),
            save_committed(1, "2026-10-30T10:00:00-02:00"),
            save_committed(2, "2026-11-01T22:40:00-02:00"),
            save_committed(3, "2026-11-02T09:15:00-02:00"),
        ];
        let state = state_with(saves, "2026-11-02T12:00:00");
        let pt = look(Lang::PtBr);
        let scene = Scene::new(&state, &pt, 1000.0, 240.0);
        let when = |i: usize| scene.tooltip_when(&state.data.saves[i]);
        assert_eq!(when(0), "30 de dez. de 2025 10:00");
        assert_eq!(when(1), "30 de out. 10:00");
        assert_eq!(when(2), "ontem 22:40");
        assert_eq!(when(3), "hoje 09:15");
        let en = look(Lang::En);
        let scene = Scene::new(&state, &en, 1000.0, 240.0);
        assert_eq!(scene.tooltip_when(&state.data.saves[1]), "Oct 30 10:00");
    }

    #[test]
    fn only_the_balls_near_the_view_are_painted() {
        // A save every six hours for thirty days: 35 px apart at the default zoom, each with
        // its own ball, laid out a screen width beyond either edge.
        let first = DateTime::parse_from_rfc3339("2026-10-03T12:00:00-03:00").expect("a date");
        let saves = (0..120)
            .map(|n| save_made(n, first + TimeDelta::hours(6 * n as i64)))
            .collect();
        let state = state_with(saves, "2026-11-02T12:00:00");
        let pt = look(Lang::PtBr);
        let scene = Scene::new(&state, &pt, 1000.0, 240.0);
        let reach = f64::from(LABEL_REACH);
        let near = |item: &&Item| (-reach..1000.0 + reach).contains(&item.x());
        assert!(scene.items.iter().any(|item| item.x() < -reach));
        let painted: Vec<&Item> = scene.balls_to_paint().collect();
        assert!(painted.iter().all(near), "{painted:?}");
        // All of those near it, but "You are here", painted on top.
        let expected = scene.items.iter().filter(near).count() - 1;
        assert_eq!(painted.len(), expected);
        assert!(
            !painted
                .iter()
                .any(|item| matches!(item, Item::Single { index: 119, .. }))
        );
    }

    /// The middle of shown row `row` in `frame`.
    fn row_y(frame: &ListBox, row: usize) -> f32 {
        frame.y + LIST_PAD + (row as f32 + 0.5) * LIST_ROW_H
    }

    #[test]
    fn group_list_lists_newest_first_under_the_group() {
        let (w, h, line_y) = (1000.0, 280.0, 118.0);
        let list = GroupList::new(10..16, 500.0, 11.0);
        assert_eq!(list.len(), 6);
        assert_eq!(list.save_at(0), Some(15));
        assert_eq!(list.save_at(5), Some(10));
        assert_eq!(list.save_at(6), None);
        let frame = list.frame(w, h, line_y);
        assert_eq!(frame.shown, 6);
        assert!(frame.y > line_y + 11.0, "{frame:?}");
        assert!(frame.y + frame.h <= h - EDGE_PAD, "{frame:?}");
        assert_close(frame.x + frame.w / 2.0, 500.0, 1e-3);
        assert_eq!(list.row_at(&frame, 500.0, row_y(&frame, 0)), Some(0));
        assert_eq!(list.row_at(&frame, 500.0, row_y(&frame, 5)), Some(5));
        assert_eq!(list.row_at(&frame, frame.x - 1.0, row_y(&frame, 0)), None);
        assert!(frame.contains(500.0, row_y(&frame, 0)));
        assert!(!frame.contains(500.0, frame.y + frame.h + 1.0));

        // Many saves in a short band: as many rows as fit, inside the widget, scrolling.
        let mut list = GroupList::new(0..50, 990.0, 13.0);
        let frame = list.frame(w, 160.0, 67.0);
        assert!(frame.shown >= 3 && frame.shown < 50, "{frame:?}");
        assert!(frame.y >= EDGE_PAD && frame.y + frame.h <= 160.0 - EDGE_PAD);
        assert!(frame.x + frame.w <= w - EDGE_PAD, "{frame:?}");
        list.scroll_by(-3.0, &frame);
        assert_eq!(list.scroll, 3);
        let x = frame.x + 5.0;
        assert_eq!(list.row_at(&frame, x, row_y(&frame, 0)), Some(3));
        list.scroll_by(-1_000.0, &frame);
        assert_eq!(list.scroll, 50 - frame.shown);
        list.scroll_by(1_000.0, &frame);
        assert_eq!(list.scroll, 0);
    }

    #[test]
    fn small_wheel_turns_add_up_in_the_group_list() {
        let mut list = GroupList::new(0..50, 500.0, 13.0);
        let frame = list.frame(1000.0, 160.0, 67.0);
        // A touchpad turns the wheel by tenths of a line: ten of them scroll one row.
        for _ in 0..9 {
            list.scroll_by(-0.1, &frame);
        }
        assert_eq!(list.scroll, 0);
        list.scroll_by(-0.1, &frame);
        assert_eq!(list.scroll, 1);
        // Back up the same way.
        for _ in 0..10 {
            list.scroll_by(0.1, &frame);
        }
        assert_eq!(list.scroll, 0);
        // A whole line still scrolls a row at once.
        list.scroll_by(-1.0, &frame);
        assert_eq!(list.scroll, 1);
    }
}
