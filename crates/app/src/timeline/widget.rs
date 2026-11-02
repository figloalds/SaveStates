//! The timeline widget: paints a [`TimelineState`] as a glowing metro line in a Raven custom
//! widget and feeds it the widget's input (spec §5 and §6.2).
//!
//! The state sits behind an `Rc` that the paint and event callbacks and the animation timers
//! share. No `RefCell` borrow is held while `on_action` runs, so the owner may call back into the
//! widget from it. Actions that come from the owner's own calls (`set_data`, `set_zoom`...) are
//! handed over on the next frame, never from inside the call.
//!
//! The owner hears of `Select`, `JumpToEnd` and `RequestOlder`. The widget repaints itself, so
//! `Redraw` stays inside. `RequestOlder` goes out once, then waits for `prepend_older` (or a
//! `set_data`) before it can go out again.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use chrono::{DateTime, FixedOffset, Local};
use savestates_core::SaveState;
use savestates_i18n::{Lang, relative_date};
use savestates_ui_raven::{
    Event, MouseButton, Painter, Rgba, Widget, custom, task, text_width, tr, tr_plural,
};

use super::model::{
    Detail, MAX_ZOOM, MIN_ZOOM, STATION_SPACING, Selection, TimelineAction, TimelineData,
    TimelineState, View, detail_for, handle_event, hit_test, needs_older_page, station_x,
    truncate_label,
};
use crate::theme;

/// The line sits at this fraction of the widget's height.
const LINE_Y_FRACTION: f32 = 0.42;
/// The saved line: a wide faint pass for the glow, then the line itself.
const GLOW_WIDTH: f32 = 6.0;
const GLOW_ALPHA: f32 = 0.25;
const LINE_WIDTH: f32 = 3.0;
/// How far past the widget's edges lines are drawn, so their round caps never show there.
const OVERDRAW: f32 = 10.0;

/// A save: a ring around a dark center that hides the line.
const STATION_R: f32 = 7.0;
const STATION_STROKE: f32 = 2.5;
/// Below this radius (far out) a station is a plain dot.
const DOT_ONLY_R: f32 = 3.5;
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

/// Names are cut to this many characters, and further when the room between stations is less.
const LABEL_CHARS: usize = 24;
/// Space kept between the labels of neighbouring stations.
const LABEL_GAP: f32 = 4.0;
/// A label narrower than this is not drawn at all.
const MIN_LABEL_W: f32 = 18.0;
/// The labels start this far below the line.
const LABEL_TOP: f32 = HERE_RING_R + 7.0;
/// "Now"'s label may spread this far left of the station when no label is beside it.
const NOW_LABEL_HALF: f32 = 60.0;
/// "Now"'s explanation wraps at this width.
const NOW_DETAIL_W: f32 = 150.0;
/// Space kept from the widget's edges.
const EDGE_PAD: f32 = 6.0;
const NAME_SIZE: f32 = 12.0;
const DETAIL_SIZE: f32 = 11.0;
const TAG_SIZE: f32 = 10.0;
const NOTE_SIZE: f32 = 11.0;
const COUNT_SIZE: f32 = 11.0;
/// The height of a line of text, per unit of font size (the theme font, Roboto).
const LINE_HEIGHT: f32 = 1.25;

/// The tooltip: shown after the pointer rests on a station for a moment.
const TOOLTIP_DELAY_S: f64 = 0.5;
const TOOLTIP_MAX_W: f32 = 320.0;
const TOOLTIP_NAME_LINES: usize = 3;
const TOOLTIP_PAD_X: f32 = 8.0;
const TOOLTIP_PAD_Y: f32 = 6.0;
const TOOLTIP_RADIUS: f32 = 6.0;

/// A new save grows in over this long, easing out.
const GROW_S: f32 = 0.25;
/// The animation repaints at this pace.
const FRAME_S: f64 = 1.0 / 60.0;

/// The timeline of one project tab. Cloning gives another handle to the same widget.
#[derive(Clone)]
pub struct TimelineWidget {
    shared: Rc<Shared>,
}

/// A new timeline widget, without a parent yet: put it in an `Overlay` or a `Border` to fill it.
/// It shows nothing until [`TimelineWidget::set_data`]. `lang` is the language of its dates.
pub fn new(
    lang: Lang,
    on_action: impl FnMut(TimelineAction) + 'static,
) -> (TimelineWidget, Widget) {
    let widget = custom::create();
    let shared = Rc::new(Shared {
        inner: RefCell::new(Inner::new(lang)),
        actions: ActionQueue {
            on_action: RefCell::new(Box::new(on_action)),
            queue: RefCell::default(),
        },
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
    (TimelineWidget { shared }, widget)
}

impl TimelineWidget {
    /// Shows `d`, keeping the view. The first call also scrolls to the end, so that "Now" is in
    /// view on open (at the first paint, once the widget's width is known).
    pub fn set_data(&self, d: TimelineData) {
        let shared = &self.shared;
        let older = {
            let mut inner = shared.inner.borrow_mut();
            inner.state.data = d;
            inner.older_in_flight = false;
            inner.forget_stale_indexes();
            if !inner.loaded {
                inner.loaded = true;
                inner.jump_to_end();
            }
            inner.claim_older_page()
        };
        shared.widget.invalidate();
        if older {
            shared.emit_later(TimelineAction::RequestOlder);
        }
    }

    /// Adds the save just made after the newest one, and grows its station in. A Quick Save
    /// saves every change, so "Now" shows none until the next `set_data`. When "Now" was in view
    /// it stays in view.
    pub fn append_saved(&self, s: SaveState) {
        let shared = &self.shared;
        let start_ticking = {
            let mut guard = shared.inner.borrow_mut();
            let inner = &mut *guard;
            let now_was_visible = inner.now_on_screen();
            inner.state.data.saves.push(s);
            inner.state.data.unsaved = 0;
            let index = inner.state.data.saves.len() - 1;
            if now_was_visible {
                inner.jump_to_end();
            }
            inner.grow = Some(Grow {
                index,
                start: Instant::now(),
            });
            !std::mem::replace(&mut inner.ticking, true)
        };
        shared.widget.invalidate();
        if start_ticking {
            shared.tick_grow();
        }
    }

    /// Sets the zoom (clamped to the model's limits), keeping the middle of the view in place.
    pub fn set_zoom(&self, z: f32) {
        if !z.is_finite() {
            return;
        }
        let shared = &self.shared;
        let older = {
            let mut inner = shared.inner.borrow_mut();
            let view = &mut inner.state.view;
            let anchor = view.width / 2.0;
            let world = view.screen_to_world(anchor);
            view.zoom = z.clamp(MIN_ZOOM, MAX_ZOOM);
            view.offset_x = world - anchor / view.zoom;
            inner.claim_older_page()
        };
        shared.widget.invalidate();
        if older {
            shared.emit_later(TimelineAction::RequestOlder);
        }
    }

    pub fn zoom(&self) -> f32 {
        self.shared.inner.borrow().state.view.zoom
    }

    /// Scrolls so that "Now" sits near the right edge.
    pub fn jump_to_end(&self) {
        let shared = &self.shared;
        let older = {
            let mut inner = shared.inner.borrow_mut();
            inner.jump_to_end();
            inner.claim_older_page()
        };
        shared.widget.invalidate();
        if older {
            shared.emit_later(TimelineAction::RequestOlder);
        }
    }

    /// Adds the page of older saves that `RequestOlder` asked for, given newest first (as
    /// `timeline_page` returns it). Every station stays where it was on screen.
    /// `more_available` tells whether saves older than this page exist.
    pub fn prepend_older(&self, page: Vec<SaveState>, more_available: bool) {
        let shared = &self.shared;
        let older = {
            let mut guard = shared.inner.borrow_mut();
            let inner = &mut *guard;
            let added = page.len();
            let state = &mut inner.state;
            state.data.prepend_older(page, &mut state.view);
            state.data.more_available = more_available;
            inner.older_in_flight = false;
            inner.hover = inner.hover.map(|i| i + added);
            if let Some(grow) = &mut inner.grow {
                grow.index += added;
            }
            inner.claim_older_page()
        };
        shared.widget.invalidate();
        if older {
            shared.emit_later(TimelineAction::RequestOlder);
        }
    }

    /// The language of the dates. The other texts follow Raven's current language.
    pub fn set_lang(&self, lang: Lang) {
        self.shared.inner.borrow_mut().lang = lang;
        self.shared.widget.invalidate();
    }
}

struct Shared {
    inner: RefCell<Inner>,
    actions: ActionQueue,
    widget: Widget,
}

/// Hands actions to the owner one at a time. An action raised while the owner's callback runs
/// (it called back into the widget) waits in the queue until the callback returns.
struct ActionQueue {
    on_action: RefCell<Box<dyn FnMut(TimelineAction)>>,
    queue: RefCell<VecDeque<TimelineAction>>,
}

impl ActionQueue {
    fn emit(&self, actions: impl IntoIterator<Item = TimelineAction>) {
        self.queue.borrow_mut().extend(actions);
        let Ok(mut on_action) = self.on_action.try_borrow_mut() else {
            return;
        };
        loop {
            let next = self.queue.borrow_mut().pop_front();
            let Some(action) = next else {
                break;
            };
            on_action(action);
        }
    }
}

#[derive(Clone, Copy)]
struct Grow {
    index: usize,
    start: Instant,
}

struct Inner {
    state: TimelineState,
    /// The language of the dates.
    lang: Lang,
    /// `set_data` ran: until then the widget shows only its background.
    loaded: bool,
    /// Scroll to the end at the next paint, which knows the width.
    jump_pending: bool,
    /// `RequestOlder` went out and its page has not come back yet.
    older_in_flight: bool,
    /// The station growing in after `append_saved`.
    grow: Option<Grow>,
    /// The animation's timer chain is running.
    ticking: bool,
    /// The left button went down on the widget and is still down.
    pressed: bool,
    /// Where the pointer is, while it is over the widget.
    pointer: Option<(f32, f32)>,
    /// The station under the pointer.
    hover: Option<usize>,
    /// Counts hover changes, so that a tooltip timer armed for an earlier one does nothing.
    hover_generation: u64,
    tooltip_shown: bool,
}

impl Inner {
    fn new(lang: Lang) -> Inner {
        let data = TimelineData {
            saves: Vec::new(),
            more_available: false,
            unsaved: 0,
            selection: Selection::Now,
            other_timelines: false,
        };
        let view = View {
            zoom: 1.0,
            offset_x: 0.0,
            width: 0.0,
        };
        Inner {
            state: TimelineState::new(data, view, 0.0),
            lang,
            loaded: false,
            jump_pending: false,
            older_in_flight: false,
            grow: None,
            ticking: false,
            pressed: false,
            pointer: None,
            hover: None,
            hover_generation: 0,
            tooltip_shown: false,
        }
    }

    /// Scrolls to the end now if the width is known, else at the next paint.
    fn jump_to_end(&mut self) {
        if self.state.view.width > 0.0 {
            let count = self.state.data.station_count();
            self.state.view.jump_to_end(count);
            self.jump_pending = false;
        } else {
            self.jump_pending = true;
        }
    }

    /// Whether to ask for the page before the oldest loaded save now. A yes marks the request
    /// as in flight, so that it goes out once.
    fn claim_older_page(&mut self) -> bool {
        if self.loaded
            && !self.older_in_flight
            && self.state.view.width > 0.0
            && needs_older_page(&self.state.view, &self.state.data)
        {
            self.older_in_flight = true;
            true
        } else {
            false
        }
    }

    /// Follows the widget's size. Returns whether the view moved.
    fn layout(&mut self, width: f32, height: f32) -> bool {
        self.state.line_y = (height * LINE_Y_FRACTION).round();
        let view = &mut self.state.view;
        let mut moved = false;
        if width != view.width {
            if view.width > 0.0 {
                // Keep what is at the right edge, where "Now" usually is, in place.
                view.offset_x += (view.width - width) / view.zoom;
            }
            view.width = width;
            moved = true;
        }
        if self.jump_pending && width > 0.0 {
            self.jump_to_end();
            moved = true;
        }
        moved
    }

    fn now_on_screen(&self) -> bool {
        let view = &self.state.view;
        let x = view.world_to_screen(station_x(self.state.data.now_index()));
        view.width > 0.0 && (0.0..=view.width).contains(&x)
    }

    /// Notes the station under the pointer. Returns the generation to arm a tooltip for, when the
    /// pointer came to rest on a new station.
    fn set_hover(&mut self, hover: Option<usize>) -> Option<u64> {
        if hover == self.hover {
            return None;
        }
        self.hover = hover;
        self.tooltip_shown = false;
        self.hover_generation = self.hover_generation.wrapping_add(1);
        (hover.is_some() && !self.pressed).then_some(self.hover_generation)
    }

    /// Drops the hover and the animation when their station is gone from new data.
    fn forget_stale_indexes(&mut self) {
        let data = &self.state.data;
        if self.hover.is_some_and(|i| i >= data.station_count()) {
            self.hover = None;
            self.tooltip_shown = false;
        }
        if self.grow.is_some_and(|g| g.index >= data.saves.len()) {
            self.grow = None;
        }
    }
}

impl Shared {
    fn paint(self: &Rc<Self>, p: &mut Painter, w: f32, h: f32) {
        let Ok(mut inner) = self.inner.try_borrow_mut() else {
            return;
        };
        let moved = inner.layout(w, h);
        let older = moved && inner.claim_older_page();
        p.push_clip(0.0, 0.0, w, h);
        p.rect(0.0, 0.0, w, h, 0.0, theme::BG);
        if inner.loaded {
            Scene::new(&inner, w, h).paint(p);
        }
        p.pop_clip();
        drop(inner);
        if older {
            self.emit_later(TimelineAction::RequestOlder);
        }
    }

    fn event(self: &Rc<Self>, ev: &Event) -> bool {
        let Ok(mut guard) = self.inner.try_borrow_mut() else {
            return false;
        };
        let inner = &mut *guard;
        if !inner.loaded {
            return false;
        }
        let before = (inner.state.view, inner.hover, inner.tooltip_shown);
        let consumed = match *ev {
            Event::MouseDown {
                x,
                y,
                button: MouseButton::Left,
                ..
            } => {
                inner.pressed = true;
                inner.tooltip_shown = false;
                inner.pointer = Some((x, y));
                true
            }
            Event::MouseUp {
                x,
                y,
                button: MouseButton::Left,
            } => {
                inner.pressed = false;
                inner.pointer = Some((x, y));
                true
            }
            Event::MouseMove { x, y, .. } => {
                inner.pointer = Some((x, y));
                true
            }
            Event::Wheel { x, y, .. } => {
                // As Raven's own tooltips do, turning the wheel hides the tooltip.
                inner.tooltip_shown = false;
                inner.pointer = Some((x, y));
                true
            }
            Event::Leave => {
                inner.pressed = false;
                inner.pointer = None;
                false
            }
            _ => false,
        };

        let actions = handle_event(&mut inner.state, ev);
        let hover = inner.pointer.and_then(|(x, y)| {
            let s = &inner.state;
            hit_test(&s.view, s.data.station_count(), x, y, s.line_y)
        });
        let arm = inner.set_hover(hover);

        let mut repaint = !actions.is_empty();
        let mut out = Vec::new();
        for action in actions {
            match action {
                TimelineAction::Redraw => {}
                TimelineAction::RequestOlder => {
                    if !inner.older_in_flight {
                        inner.older_in_flight = true;
                        out.push(action);
                    }
                }
                TimelineAction::Select(_) | TimelineAction::JumpToEnd => out.push(action),
            }
        }
        repaint |= (inner.state.view, inner.hover, inner.tooltip_shown) != before;
        drop(guard);

        if repaint {
            self.widget.invalidate();
        }
        if let Some(generation) = arm {
            self.arm_tooltip(generation);
        }
        self.actions.emit(out);
        consumed
    }

    /// Hands `action` to the owner on the next frame, outside whatever call raised it.
    fn emit_later(self: &Rc<Self>, action: TimelineAction) {
        let shared = self.clone();
        task::timer(0.0, false, move || shared.actions.emit([action]));
    }

    /// Shows the tooltip after a moment, if the pointer is still on the same station by then.
    fn arm_tooltip(self: &Rc<Self>, generation: u64) {
        let shared = self.clone();
        task::timer(TOOLTIP_DELAY_S, false, move || {
            let shown = shared.inner.try_borrow_mut().is_ok_and(|mut inner| {
                let due =
                    inner.hover_generation == generation && inner.hover.is_some() && !inner.pressed;
                inner.tooltip_shown |= due;
                due
            });
            if shown {
                shared.widget.invalidate();
            }
        });
    }

    /// Repaints at the animation's pace until the growing station is full size. One-shot timers,
    /// each arming the next: they are freed once they ran.
    fn tick_grow(self: &Rc<Self>) {
        let shared = self.clone();
        task::timer(FRAME_S, false, move || {
            let again = match shared.inner.try_borrow_mut() {
                Ok(mut inner) => {
                    let running = inner
                        .grow
                        .is_some_and(|g| g.start.elapsed().as_secs_f32() < GROW_S);
                    if !running {
                        inner.grow = None;
                        inner.ticking = false;
                    }
                    running
                }
                // Busy: try again on the next tick.
                Err(_) => true,
            };
            shared.widget.invalidate();
            if again {
                shared.tick_grow();
            }
        });
    }
}

/// One paint of the timeline: what the state looks like at this size and moment.
struct Scene<'a> {
    inner: &'a Inner,
    data: &'a TimelineData,
    view: View,
    w: f32,
    h: f32,
    line_y: f32,
    /// Screen distance between two stations.
    spacing: f32,
    detail: Detail,
    range: Range<usize>,
    here: Option<usize>,
    now_index: usize,
    hover: Option<usize>,
    clock: Instant,
    now: DateTime<FixedOffset>,
}

impl<'a> Scene<'a> {
    fn new(inner: &'a Inner, w: f32, h: f32) -> Scene<'a> {
        let state = &inner.state;
        let data = &state.data;
        let view = state.view;
        Scene {
            inner,
            data,
            view,
            w,
            h,
            line_y: state.line_y,
            spacing: STATION_SPACING * view.zoom,
            detail: detail_for(view.zoom),
            range: view.visible_range(data.station_count()),
            here: data.you_are_here(),
            now_index: data.now_index(),
            hover: inner.hover.filter(|_| !inner.pressed),
            clock: Instant::now(),
            now: Local::now().fixed_offset(),
        }
    }

    fn paint(&self, p: &mut Painter) {
        self.line(p);
        self.connector(p);
        self.stations(p);
        self.here_station(p);
        self.now_station(p);
        self.labels(p);
        self.now_label(p);
        self.here_label(p);
        self.note(p);
        self.tooltip(p);
    }

    /// The screen x of station `i`.
    fn x(&self, i: usize) -> f32 {
        self.view.world_to_screen(station_x(i))
    }

    /// How far station `i` has grown in, from 0 to 1.
    fn grown(&self, i: usize) -> f32 {
        match self.inner.grow {
            Some(g) if g.index == i => {
                let t = self.clock.saturating_duration_since(g.start).as_secs_f32() / GROW_S;
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

    fn save_tooltip_lines(&self, save: &SaveState) -> Vec<(String, f32, Rgba)> {
        let measure = |s: &str| text_width(NAME_SIZE, s);
        let width = TOOLTIP_MAX_W - 2.0 * TOOLTIP_PAD_X;
        let mut lines: Vec<(String, f32, Rgba)> =
            wrap_lines(&save.name, width, TOOLTIP_NAME_LINES, measure)
                .into_iter()
                .map(|line| (line, NAME_SIZE, theme::TEXT))
                .collect();
        let date = relative_date(self.inner.lang, save.time, self.now);
        let files = tr_plural("timeline.files", i64::from(save.files_changed));
        lines.push((date, DETAIL_SIZE, theme::MUTED_TEXT));
        lines.push((files, DETAIL_SIZE, theme::MUTED_TEXT));
        lines
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

    /// The saved line, from the oldest save (or the left edge, when older saves are still to
    /// load) to "You are here". It reaches a station that is growing in as it grows.
    fn line(&self, p: &mut Painter) {
        let Some(here) = self.here else {
            return;
        };
        let mut end = self.x(here);
        if here > 0 {
            let before = self.x(here - 1);
            end = before + (end - before) * self.grown(here);
        }
        let start = if self.data.more_available {
            -OVERDRAW
        } else {
            self.x(0)
        };
        let (x0, x1) = (start.max(-OVERDRAW), end.min(self.w + OVERDRAW));
        if x0 < x1 {
            let y = self.line_y;
            p.line(
                x0,
                y,
                x1,
                y,
                GLOW_WIDTH,
                with_alpha(theme::LINE, GLOW_ALPHA),
            );
            p.line(x0, y, x1, y, LINE_WIDTH, theme::LINE);
        }
    }

    /// The dashed amber connector from "You are here" to "Now".
    fn connector(&self, p: &mut Painter) {
        let Some(here) = self.here else {
            return;
        };
        let from = (self.x(here) + HERE_RING_R + 3.0).max(-OVERDRAW);
        let to = (self.x(self.now_index) - NOW_R - 3.0).min(self.w + OVERDRAW);
        let color = with_alpha(theme::UNSAVED, 0.85 * self.now_alpha());
        for (x0, x1) in dash_spans(from, to, CONNECTOR_DASH, CONNECTOR_GAP) {
            p.line(x0, self.line_y, x1, self.line_y, CONNECTOR_WIDTH, color);
        }
    }

    /// Every visible save but "You are here".
    fn stations(&self, p: &mut Painter) {
        let r = station_radius(self.spacing);
        let y = self.line_y;
        for i in self.range.clone() {
            if i == self.now_index || Some(i) == self.here {
                continue;
            }
            let (cx, r) = (self.x(i), r * self.grown(i));
            if self.data.selection == Selection::Station(i) {
                halo(p, cx, y, r, theme::LINE);
            } else if self.hover == Some(i) {
                p.circle(cx, y, r + HOVER_EXTRA, with_alpha(theme::TEXT, HOVER_ALPHA));
            }
            if r < DOT_ONLY_R {
                p.circle(cx, y, r, theme::LINE);
            } else {
                p.circle(cx, y, r, theme::BG);
                ring(p, cx, y, r, STATION_STROKE.min(r * 0.36), theme::LINE);
            }
        }
    }

    /// The latest save: a filled dot, a dark gap and a faint ring.
    fn here_station(&self, p: &mut Painter) {
        let Some(here) = self.here.filter(|i| self.range.contains(i)) else {
            return;
        };
        let (cx, y, g) = (self.x(here), self.line_y, self.grown(here));
        if self.data.selection == Selection::Station(here) {
            halo(p, cx, y, HERE_RING_R * g, theme::LINE);
        } else if self.hover == Some(here) {
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
        if !self.range.contains(&self.now_index) {
            return;
        }
        let (cx, y, alpha) = (self.x(self.now_index), self.line_y, self.now_alpha());
        let color = with_alpha(theme::UNSAVED, alpha);
        if self.data.selection == Selection::Now {
            halo(p, cx, y, NOW_R, color);
        } else if self.hover == Some(self.now_index) {
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

    /// The names (and, close in, dates and file counts) under the saves.
    fn labels(&self, p: &mut Painter) {
        if self.detail == Detail::DotsOnly {
            return;
        }
        let max_w = self.spacing - 2.0 * LABEL_GAP;
        if max_w < MIN_LABEL_W {
            return;
        }
        let name_width = |s: &str| text_width(NAME_SIZE, s);
        let detail_width = |s: &str| text_width(DETAIL_SIZE, s);
        for i in self.range.clone() {
            let Some(save) = self.data.saves.get(i) else {
                continue;
            };
            let cx = self.x(i);
            let mut y = self.line_y + LABEL_TOP;
            let name = fit_label(&save.name, LABEL_CHARS, max_w, name_width);
            centered_text(p, NAME_SIZE, cx, y, &name, theme::TEXT);
            if self.detail == Detail::Full {
                y += line_height(NAME_SIZE);
                let date = relative_date(self.inner.lang, save.time, self.now);
                let date = fit_label(&date, usize::MAX, max_w, detail_width);
                centered_text(p, DETAIL_SIZE, cx, y, &date, theme::MUTED_TEXT);
                y += line_height(DETAIL_SIZE);
                let n = i64::from(save.files_changed);
                let files = fit_label(
                    &tr_plural("timeline.files", n),
                    usize::MAX,
                    max_w,
                    detail_width,
                );
                centered_text(p, DETAIL_SIZE, cx, y, &files, theme::MUTED_TEXT);
            }
        }
    }

    /// "Now", and what it holds, under the "Now" station. It may spread right, where nothing
    /// else is, but not left into the label of "You are here".
    fn now_label(&self, p: &mut Painter) {
        if !self.range.contains(&self.now_index) {
            return;
        }
        let cx = self.x(self.now_index);
        let left_room = if self.detail == Detail::DotsOnly || self.here.is_none() {
            NOW_LABEL_HALF
        } else {
            (self.spacing / 2.0 - LABEL_GAP).max(0.0)
        };
        let left = cx - left_room;
        let right = (self.w - EDGE_PAD).max(cx + left_room);
        let max_w = right - left;
        let alpha = self.now_alpha();
        let mut y = self.line_y + LABEL_TOP;

        let title = fit_label(&tr("timeline.now"), usize::MAX, max_w, |s| {
            text_width(NAME_SIZE, s)
        });
        let color = with_alpha(theme::UNSAVED, alpha);
        span_text(p, NAME_SIZE, cx, left, right, y, &title, color);
        if self.detail == Detail::DotsOnly {
            return;
        }
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
        let Some(here) = self.here.filter(|i| self.range.contains(i)) else {
            return;
        };
        let text = tr("timeline.you_are_here").to_uppercase();
        let y = self.line_y - HERE_RING_R - 6.0 - line_height(TAG_SIZE);
        centered_text(p, TAG_SIZE, self.x(here), y, &text, theme::LINE);
    }

    /// The note about timelines this view does not show, at the top left.
    fn note(&self, p: &mut Painter) {
        if !self.data.other_timelines {
            return;
        }
        let max_w = self.w - 4.0 * EDGE_PAD;
        let text = fit_label(
            &tr("timeline.other_timelines_note"),
            usize::MAX,
            max_w,
            |s| text_width(NOTE_SIZE, s),
        );
        let (x, y, tw) = (2.0 * EDGE_PAD, EDGE_PAD, text_width(NOTE_SIZE, &text));
        // A backing, so that stations passing under the note do not muddle it.
        let backing = with_alpha(theme::BG, 0.85);
        p.rect(
            x - 4.0,
            y - 2.0,
            tw + 8.0,
            line_height(NOTE_SIZE) + 4.0,
            4.0,
            backing,
        );
        p.text(NOTE_SIZE, x, y, &text, theme::MUTED_TEXT);
    }

    /// The full name, date and file count of the station under the pointer, next to the pointer.
    fn tooltip(&self, p: &mut Painter) {
        if !self.inner.tooltip_shown {
            return;
        }
        let (Some(i), Some((px, py))) = (self.hover, self.inner.pointer) else {
            return;
        };
        let lines = match self.data.saves.get(i) {
            Some(save) => self.save_tooltip_lines(save),
            None => self.now_tooltip_lines(),
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

/// The radius of a save's station: 7, smaller when zoomed so far out that neighbours would
/// touch.
fn station_radius(spacing: f32) -> f32 {
    STATION_R.min(spacing * 0.32).max(1.5)
}

/// Eases out (cubic): fast at first, settling at 1. `t` is clamped to 0..1.
fn ease_out(t: f32) -> f32 {
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
    use super::super::model::NOW_MARGIN_PX;
    use super::*;
    use chrono::TimeZone;
    use savestates_core::SaveId;
    use savestates_ui_raven::{Backend, Window, draw_frame, init, shutdown, wait_frame};

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

    #[test]
    fn stations_shrink_only_when_neighbours_would_touch() {
        assert_eq!(station_radius(STATION_SPACING), STATION_R);
        assert_eq!(station_radius(STATION_SPACING * 0.2), STATION_R);
        assert_close(station_radius(11.0), 3.52, 1e-4);
        assert_eq!(station_radius(1.0), 1.5);
    }

    fn save(n: usize) -> SaveState {
        SaveState {
            id: SaveId(format!("{n:040x}")),
            name: format!("Save {n}"),
            time: FixedOffset::west_opt(3 * 3600)
                .unwrap()
                .timestamp_opt(1_700_000_000 + n as i64 * 60, 0)
                .unwrap(),
            files_changed: 1 + (n % 3) as u32,
        }
    }

    fn data(saves: Range<usize>, more_available: bool) -> TimelineData {
        TimelineData {
            saves: saves.map(save).collect(),
            more_available,
            unsaved: 3,
            selection: Selection::Now,
            other_timelines: true,
        }
    }

    /// Runs frames for `seconds`: Raven fires timers during frames.
    fn run_for(seconds: f64) {
        let start = Instant::now();
        while start.elapsed().as_secs_f64() < seconds {
            wait_frame(0.005);
        }
    }

    fn click(win: Window, x: f32, y: f32) {
        win.inject_mouse(x, y, Some(true));
        win.inject_mouse(x, y, Some(false));
    }

    /// Everything that needs Raven is in this one test: Raven's UI thread is the thread that
    /// called `init`. Input goes through Raven's real event path on the NULL backend.
    #[test]
    fn the_widget_on_the_null_backend() {
        use TimelineAction::{JumpToEnd, RequestOlder, Select};

        init(Backend::Null).expect("init on the NULL backend");
        let win = Window::from_xml(
            r#"<Window Title="t" Width="1000" Height="240"><Overlay Name="host"/></Window>"#,
        )
        .expect("window");
        let actions: Rc<RefCell<Vec<TimelineAction>>> = Rc::default();
        let slot: Rc<RefCell<Option<TimelineWidget>>> = Rc::default();
        let (tw, widget) = new(Lang::PtBr, {
            let (actions, slot) = (actions.clone(), slot.clone());
            move |action| {
                actions.borrow_mut().push(action);
                // The owner may call back into the widget from here.
                if let Some(tw) = slot.borrow().as_ref() {
                    tw.set_lang(Lang::PtBr);
                    let _ = tw.zoom();
                }
            }
        });
        *slot.borrow_mut() = Some(tw.clone());
        win.find("host").expect("host").add_child(widget);

        // The first data scrolls "Now" into view, at the first paint.
        tw.set_data(data(100..600, true));
        run_for(0.05);
        let (wx, wy, ww, wh) = widget.rect();
        assert!(ww > 0.0 && wh > 0.0);
        let line_y = wy + (wh * LINE_Y_FRACTION).round();
        let view = || tw.shared.inner.borrow().state.view;
        let screen_x = |i: usize| wx + view().world_to_screen(station_x(i));
        assert_eq!(view().width, ww);
        assert_close(screen_x(500), wx + ww - NOW_MARGIN_PX, 0.01);
        assert!(actions.borrow().is_empty());

        // A click selects a station.
        click(win, screen_x(498), line_y);
        assert_eq!(actions.take(), [Select(Selection::Station(498))]);

        // After a pan, a double click on "Now" selects it and jumps back to the end.
        win.inject_wheel(wx + 500.0, line_y, 1.0, false);
        let now_x = screen_x(500);
        assert_close(now_x, wx + ww - NOW_MARGIN_PX + 60.0, 0.01);
        click(win, now_x, line_y);
        click(win, now_x, line_y);
        assert_eq!(actions.take(), [Select(Selection::Now), JumpToEnd]);
        assert_close(screen_x(500), wx + ww - NOW_MARGIN_PX, 0.01);

        // Panning far into the past asks for the older page once, however many events follow.
        tw.set_zoom(0.1);
        for lines in [100.0, 1.0, 1.0] {
            win.inject_wheel(wx + 500.0, line_y, lines, false);
        }
        assert_eq!(actions.take(), [RequestOlder]);
        run_for(0.05);
        assert!(actions.borrow().is_empty());

        // A page arrives: every station stays where it was on screen. Still near the oldest
        // save with more to load, the next request comes on the next frame, not during the call.
        let k = view().visible_range(501).start + 5;
        let before = screen_x(k);
        tw.prepend_older((90..100).rev().map(save).collect(), true);
        assert_close(screen_x(k + 10), before, 0.01);
        assert!(actions.borrow().is_empty());
        run_for(0.05);
        assert_eq!(actions.take(), [RequestOlder]);
        tw.prepend_older((0..90).rev().map(save).collect(), false);
        assert_close(screen_x(k + 100), before, 0.01);
        {
            let inner = tw.shared.inner.borrow();
            let saves = &inner.state.data.saves;
            assert_eq!(saves.len(), 600);
            assert_eq!(saves[0].name, "Save 0");
            assert_eq!(saves[k + 100].name, format!("Save {}", k + 100));
            assert!(!inner.older_in_flight);
        }
        run_for(0.05);
        assert!(actions.borrow().is_empty());

        // Ctrl+wheel zooms; set_zoom clamps and ignores nonsense.
        tw.set_zoom(1.0);
        win.inject_wheel(wx + 500.0, line_y, 1.0, true);
        assert_close(tw.zoom(), 1.15, 1e-4);
        tw.set_zoom(0.7);
        assert_eq!(tw.zoom(), 0.7);
        tw.set_zoom(50.0);
        assert_eq!(tw.zoom(), MAX_ZOOM);
        tw.set_zoom(f32::NAN);
        assert_eq!(tw.zoom(), MAX_ZOOM);

        // Resting on a station shows its tooltip after a moment; pressing hides it.
        tw.set_zoom(1.0);
        tw.jump_to_end();
        run_for(0.02);
        win.inject_mouse(screen_x(597), line_y, None);
        assert_eq!(tw.shared.inner.borrow().hover, Some(597));
        assert!(!tw.shared.inner.borrow().tooltip_shown);
        run_for(TOOLTIP_DELAY_S + 0.2);
        assert!(tw.shared.inner.borrow().tooltip_shown);
        win.inject_mouse(screen_x(597), line_y, Some(true));
        assert!(!tw.shared.inner.borrow().tooltip_shown);
        win.inject_mouse(screen_x(597), line_y, Some(false));
        assert_eq!(actions.take(), [Select(Selection::Station(597))]);

        // A new save grows in; "Now" stays in view and shows nothing unsaved.
        tw.append_saved(save(600));
        {
            let inner = tw.shared.inner.borrow();
            assert_eq!(inner.state.data.saves.len(), 601);
            assert_eq!(inner.state.data.unsaved, 0);
            assert_eq!(inner.grow.map(|g| g.index), Some(600));
        }
        assert_close(screen_x(601), wx + ww - NOW_MARGIN_PX, 0.01);
        run_for(f64::from(GROW_S) + 0.15);
        assert!(tw.shared.inner.borrow().grow.is_none());
        assert!(!tw.shared.inner.borrow().ticking);

        // Every level of detail paints, with or without saves (it must simply not fail).
        for zoom in [MIN_ZOOM, 0.3, 0.6, 1.0, MAX_ZOOM] {
            tw.set_zoom(zoom);
            draw_frame();
        }
        tw.set_data(TimelineData {
            saves: Vec::new(),
            more_available: false,
            unsaved: 0,
            selection: Selection::Now,
            other_timelines: false,
        });
        draw_frame();
        assert!(actions.borrow().is_empty());

        shutdown();
    }
}
