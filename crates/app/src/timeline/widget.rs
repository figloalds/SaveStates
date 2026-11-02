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
//!
//! Until Task 5 paints the day grid, a group is drawn as a plain ring, and `Animate` and
//! `OpenGroupList` are not acted on yet.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::rc::Rc;
use std::time::Instant;

use chrono::{DateTime, FixedOffset, Local, NaiveDateTime};
use savestates_core::SaveState;
use savestates_i18n::{Lang, relative_date};
use savestates_ui_raven::{
    Event, MouseButton, Painter, Rgba, Widget, custom, task, text_width, tr, tr_plural,
};

use super::layout::{Hit, Item, hit_test};
use super::model::{
    Selection, TimelineAction, TimelineData, TimelineState, handle_event, needs_older_page,
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
            inner.state.set_now(now_local());
            inner.state.replace_data(d);
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
    #[allow(dead_code)] // Task 17's Quick Save calls it.
    pub fn append_saved(&self, s: SaveState) {
        let shared = &self.shared;
        let start_ticking = {
            let mut guard = shared.inner.borrow_mut();
            let inner = &mut *guard;
            let now_was_visible = inner.now_on_screen();
            let mut data = inner.state.data.clone();
            data.saves.push(s);
            data.unsaved = 0;
            inner.state.set_now(now_local());
            inner.state.replace_data(data);
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

    /// Sets the zoom, the width of a day in px (clamped to the zoom range), keeping the middle
    /// of the view in place, and ends a temporary zoom. Task 5 renames it `set_day_px`.
    pub fn set_zoom(&self, z: f32) {
        if !(z.is_finite() && z > 0.0) {
            return;
        }
        let shared = &self.shared;
        let older = {
            let mut inner = shared.inner.borrow_mut();
            let state = &mut inner.state;
            let view = &mut state.view;
            view.zoom_at(f64::from(z) / view.day_px, view.width / 2.0);
            state.temp.accept();
            inner.claim_older_page()
        };
        shared.widget.invalidate();
        if older {
            shared.emit_later(TimelineAction::RequestOlder);
        }
    }

    /// The zoom: the width of a day in px. Task 5 renames it `day_px`.
    pub fn zoom(&self) -> f32 {
        self.shared.inner.borrow().state.view.day_px as f32
    }

    /// How many saves are loaded (the first page plus the older pages added since).
    pub fn save_count(&self) -> usize {
        self.shared.inner.borrow().state.data.saves.len()
    }

    /// The loaded save at index `i`, oldest first: the one `Selection::Station(i)` selects.
    pub fn save_at(&self, i: usize) -> Option<SaveState> {
        self.shared.inner.borrow().state.data.saves.get(i).cloned()
    }

    /// Selects a station, as a click would, without telling the owner (who asked for it).
    pub fn set_selection(&self, selection: Selection) {
        self.shared.inner.borrow_mut().state.data.selection = selection;
        self.shared.widget.invalidate();
    }

    /// Updates what "Now" counts and the other-timelines note, keeping the saves, the view and
    /// the selection: for a refresh that found the project still at the same save.
    pub fn set_unsaved(&self, unsaved: usize, other_timelines: bool) {
        {
            let mut inner = self.shared.inner.borrow_mut();
            inner.state.data.unsaved = unsaved;
            inner.state.data.other_timelines = other_timelines;
        }
        self.shared.widget.invalidate();
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
    /// `timeline_page` returns it). Every save stays where it was on screen.
    /// `more_available` tells whether saves older than this page exist.
    pub fn prepend_older(&self, page: Vec<SaveState>, more_available: bool) {
        let shared = &self.shared;
        let older = {
            let mut guard = shared.inner.borrow_mut();
            let inner = &mut *guard;
            let added = page.len();
            inner.state.prepend_older(page, more_available);
            inner.older_in_flight = false;
            inner.hover = inner.hover.take().map(|hit| match hit {
                Hit::Save(i) => Hit::Save(i + added),
                Hit::Group(m) => Hit::Group(m.start + added..m.end + added),
                Hit::Now => Hit::Now,
            });
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
    #[allow(dead_code)] // Task 18's language setting calls it.
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
    /// What is under the pointer.
    hover: Option<Hit>,
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
        let to_local = |t: DateTime<FixedOffset>| t.with_timezone(&Local).naive_local();
        Inner {
            state: TimelineState::new(data, 0.0, 0.0, to_local, now_local()),
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
            self.state.jump_to_end();
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
            && needs_older_page(&self.state)
        {
            self.older_in_flight = true;
            true
        } else {
            false
        }
    }

    /// Follows the widget's size. Returns whether the view moved.
    fn layout(&mut self, width: f32, height: f32) -> bool {
        self.state.line_y = f64::from((height * LINE_Y_FRACTION).round());
        let width = f64::from(width);
        let mut moved = false;
        if width != self.state.view.width {
            // What is at the right edge, where "Agora" usually is, stays in place.
            self.state.set_width(width);
            moved = true;
        }
        if self.jump_pending && width > 0.0 {
            self.jump_to_end();
            moved = true;
        }
        moved
    }

    fn now_on_screen(&self) -> bool {
        let width = self.state.view.width;
        width > 0.0 && (0.0..=width).contains(&self.state.now_screen_x())
    }

    /// Notes what is under the pointer. Returns the generation to arm a tooltip for, when the
    /// pointer came to rest on something new.
    fn set_hover(&mut self, hover: Option<Hit>) -> Option<u64> {
        if hover == self.hover {
            return None;
        }
        let some = hover.is_some();
        self.hover = hover;
        self.tooltip_shown = false;
        self.hover_generation = self.hover_generation.wrapping_add(1);
        (some && !self.pressed).then_some(self.hover_generation)
    }

    /// Drops the hover and the animation when their saves are gone from new data.
    fn forget_stale_indexes(&mut self) {
        let count = self.state.data.saves.len();
        let stale = match &self.hover {
            Some(Hit::Save(i)) => *i >= count,
            Some(Hit::Group(members)) => members.end > count,
            Some(Hit::Now) | None => false,
        };
        if stale {
            self.hover = None;
            self.tooltip_shown = false;
        }
        if self.grow.is_some_and(|g| g.index >= count) {
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
        let before = (
            inner.state.view,
            inner.state.temp,
            inner.hover.clone(),
            inner.tooltip_shown,
        );
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
            let (x, y) = (f64::from(x), f64::from(y));
            hit_test(&s.items(), s.now_screen_x(), x, y, s.line_y)
        });
        let arm = inner.set_hover(hover);

        let mut repaint = !actions.is_empty();
        let mut out = Vec::new();
        for action in actions {
            match action {
                TimelineAction::Redraw => {}
                // Task 5 animates the view (it already moved) and lists the group's saves.
                TimelineAction::Animate { .. } | TimelineAction::OpenGroupList(_) => {}
                TimelineAction::RequestOlder => {
                    if !inner.older_in_flight {
                        inner.older_in_flight = true;
                        out.push(action);
                    }
                }
                TimelineAction::Select(_) | TimelineAction::JumpToEnd => out.push(action),
            }
        }
        let after = (
            inner.state.view,
            inner.state.temp,
            inner.hover.clone(),
            inner.tooltip_shown,
        );
        repaint |= after != before;
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
        task::timer(0.0, false, move || shared.actions.emit([action.clone()]));
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
    state: &'a TimelineState,
    data: &'a TimelineData,
    w: f32,
    h: f32,
    line_y: f32,
    /// The balls near the view, in index order.
    items: Vec<Item>,
    here: Option<usize>,
    /// The screen x of "Now".
    now_x: f32,
    hover: Option<&'a Hit>,
    clock: Instant,
    now: DateTime<FixedOffset>,
}

impl<'a> Scene<'a> {
    fn new(inner: &'a Inner, w: f32, h: f32) -> Scene<'a> {
        let state = &inner.state;
        Scene {
            inner,
            state,
            data: &state.data,
            w,
            h,
            line_y: state.line_y as f32,
            items: state.items(),
            here: state.data.you_are_here(),
            now_x: state.now_screen_x() as f32,
            hover: inner.hover.as_ref().filter(|_| !inner.pressed),
            clock: Instant::now(),
            now: Local::now().fixed_offset(),
        }
    }

    fn paint(&self, p: &mut Painter) {
        self.line(p);
        self.connector(p);
        self.balls(p);
        self.here_station(p);
        self.now_station(p);
        self.labels(p);
        self.now_label(p);
        self.here_label(p);
        self.note(p);
        self.tooltip(p);
    }

    /// The screen x of save `i`.
    fn x(&self, i: usize) -> f32 {
        let state = self.state;
        state
            .xs()
            .get(i)
            .map_or(f32::NAN, |&x| state.view.world_to_screen(x) as f32)
    }

    /// "You are here", when it has a ball near the view.
    fn here_in_view(&self) -> Option<usize> {
        let here = self.here?;
        let laid_out = self.items.iter().any(|item| match *item {
            Item::Single { index, .. } => index == here,
            Item::Group { .. } => false,
        });
        laid_out.then_some(here)
    }

    /// Whether "Now" or its label can show.
    fn now_in_view(&self) -> bool {
        let margin = 2.0 * NOW_LABEL_HALF;
        (-margin..self.w + margin).contains(&self.now_x)
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
        let to = (self.now_x - NOW_R - 3.0).min(self.w + OVERDRAW);
        let color = with_alpha(theme::UNSAVED, 0.85 * self.now_alpha());
        for (x0, x1) in dash_spans(from, to, CONNECTOR_DASH, CONNECTOR_GAP) {
            p.line(x0, self.line_y, x1, self.line_y, CONNECTOR_WIDTH, color);
        }
    }

    /// Every ball near the view but "You are here". A group is a plain ring until Task 5 paints
    /// group balls.
    fn balls(&self, p: &mut Painter) {
        let y = self.line_y;
        for item in &self.items {
            match *item {
                Item::Single { index, x } => {
                    if Some(index) == self.here {
                        continue;
                    }
                    let (cx, r) = (x as f32, STATION_R * self.grown(index));
                    if self.data.selection == Selection::Station(index) {
                        halo(p, cx, y, r, theme::LINE);
                    } else if self.hover == Some(&Hit::Save(index)) {
                        p.circle(cx, y, r + HOVER_EXTRA, with_alpha(theme::TEXT, HOVER_ALPHA));
                    }
                    p.circle(cx, y, r, theme::BG);
                    ring(p, cx, y, r, STATION_STROKE.min(r * 0.36), theme::LINE);
                }
                Item::Group {
                    ref members,
                    x,
                    radius,
                } => {
                    let (cx, r) = (x as f32, radius as f32);
                    if matches!(self.hover, Some(Hit::Group(m)) if m == members) {
                        p.circle(cx, y, r + HOVER_EXTRA, with_alpha(theme::TEXT, HOVER_ALPHA));
                    }
                    p.circle(cx, y, r, theme::BG);
                    ring(p, cx, y, r, STATION_STROKE, theme::LINE);
                }
            }
        }
    }

    /// The latest save: a filled dot, a dark gap and a faint ring.
    fn here_station(&self, p: &mut Painter) {
        let Some(here) = self.here_in_view() else {
            return;
        };
        let (cx, y, g) = (self.x(here), self.line_y, self.grown(here));
        if self.data.selection == Selection::Station(here) {
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
        let (cx, y, alpha) = (self.now_x, self.line_y, self.now_alpha());
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

    /// The name, date and file count under each single save, each label no wider than the room
    /// to its neighbouring balls ("Now" after the last one).
    fn labels(&self, p: &mut Painter) {
        let name_width = |s: &str| text_width(NAME_SIZE, s);
        let detail_width = |s: &str| text_width(DETAIL_SIZE, s);
        let mut xs: Vec<f32> = self.items.iter().map(|item| item.x() as f32).collect();
        xs.push(self.now_x);
        for (k, item) in self.items.iter().enumerate() {
            let Item::Single { index, .. } = *item else {
                continue;
            };
            let Some(save) = self.data.saves.get(index) else {
                continue;
            };
            let cx = xs[k];
            let before = k.checked_sub(1).map_or(f32::INFINITY, |j| cx - xs[j]);
            let max_w = before.min(xs[k + 1] - cx) - 2.0 * LABEL_GAP;
            if max_w < MIN_LABEL_W {
                continue;
            }
            let mut y = self.line_y + LABEL_TOP;
            let name = fit_label(&save.name, LABEL_CHARS, max_w, name_width);
            centered_text(p, NAME_SIZE, cx, y, &name, theme::TEXT);
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

    /// "Now", and what it holds, under the "Now" station. It may spread right, where nothing
    /// else is, but not left into the label of "You are here".
    fn now_label(&self, p: &mut Painter) {
        if !self.now_in_view() {
            return;
        }
        let cx = self.now_x;
        let left_room = match self.here {
            None => NOW_LABEL_HALF,
            Some(here) => ((cx - self.x(here)) / 2.0 - LABEL_GAP).max(0.0),
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
        let (Some(hover), Some((px, py))) = (self.hover, self.inner.pointer) else {
            return;
        };
        let lines = match hover {
            Hit::Save(i) => match self.data.saves.get(*i) {
                Some(save) => self.save_tooltip_lines(save),
                None => return,
            },
            Hit::Now => self.now_tooltip_lines(),
            // Task 5 paints the group tooltip.
            Hit::Group(_) => return,
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

/// The computer's local time, for the model.
fn now_local() -> NaiveDateTime {
    Local::now().naive_local()
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
    use std::ops::Range;

    use super::super::axis::{DEFAULT_DAY_PX, MAX_DAY_PX, MIN_DAY_PX};
    use super::*;
    use chrono::TimeDelta;
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

    /// "Save {n}", made `n` hours after `base`.
    fn save_at(n: usize, base: DateTime<FixedOffset>) -> SaveState {
        let time = base + TimeDelta::hours(n as i64);
        SaveState {
            id: SaveId(format!("{n:040x}")),
            name: format!("Save {n}"),
            time,
            committed: time,
            files_changed: 1 + (n % 3) as u32,
        }
    }

    fn data(
        saves: Range<usize>,
        more_available: bool,
        base: DateTime<FixedOffset>,
    ) -> TimelineData {
        TimelineData {
            saves: saves.map(|n| save_at(n, base)).collect(),
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

        // Saves an hour apart: "Save 600" an hour ago, "Save 100" 21 days ago.
        let base = Local::now().fixed_offset() - TimeDelta::hours(601);
        let save = |n: usize| save_at(n, base);

        // The first data scrolls "Now" into view, at the first paint.
        tw.set_data(data(100..600, true, base));
        run_for(0.05);
        let (wx, wy, ww, wh) = widget.rect();
        assert!(ww > 0.0 && wh > 0.0);
        let line_y = wy + (wh * LINE_Y_FRACTION).round();
        let view = || tw.shared.inner.borrow().state.view;
        let screen_x = |i: usize| {
            let inner = tw.shared.inner.borrow();
            wx + inner.state.view.world_to_screen(inner.state.xs()[i]) as f32
        };
        let now_x = || wx + tw.shared.inner.borrow().state.now_screen_x() as f32;
        // Where "Now" sits after jumping to the end.
        let end_x = wx + ww - 80.0;
        assert_eq!(view().width, f64::from(ww));
        assert_eq!(view().day_px, DEFAULT_DAY_PX);
        assert_close(now_x(), end_x, 0.01);
        assert!(actions.borrow().is_empty());

        // At 1,440 px a day an hour is 60 px: every save has its own ball.
        tw.set_zoom(1440.0);
        tw.jump_to_end();
        assert_close(tw.zoom(), 1440.0, 1e-3);
        assert_close(now_x(), end_x, 0.01);

        // A click selects a save.
        click(win, screen_x(498), line_y);
        assert_eq!(actions.take(), [Select(Selection::Station(498))]);

        // After a drag, a double click on "Now" selects it and jumps back to the end.
        let x = screen_x(497) + 30.0;
        win.inject_mouse(x, line_y, None);
        win.inject_mouse(x, line_y, Some(true));
        win.inject_mouse(x + 60.0, line_y, None);
        win.inject_mouse(x + 60.0, line_y, Some(false));
        assert!(actions.borrow().is_empty());
        assert_close(now_x(), end_x + 60.0, 0.01);
        let x = now_x();
        click(win, x, line_y);
        click(win, x, line_y);
        assert_eq!(actions.take(), [Select(Selection::Now), JumpToEnd]);
        assert_close(now_x(), end_x, 0.01);

        // Zooming far out asks for the older page once, however many events follow.
        for lines in [-30.0, -1.0, -1.0] {
            win.inject_wheel(wx + 500.0, line_y, lines, false);
        }
        assert_eq!(actions.take(), [RequestOlder]);
        run_for(0.05);
        assert!(actions.borrow().is_empty());

        // A page arrives: every save stays where it was on screen. Still near the oldest save
        // with more to load, the next request comes on the next frame, not during the call.
        let k = 5;
        let before = screen_x(k);
        tw.prepend_older((90..100).rev().map(save).collect(), true);
        assert_close(screen_x(k + 10), before, 0.01);
        assert!(actions.borrow().is_empty());
        run_for(0.05);
        assert_eq!(actions.take(), [RequestOlder]);
        tw.prepend_older((0..90).rev().map(save).collect(), false);
        assert_close(screen_x(k + 100), before, 0.01);
        assert_eq!(tw.save_count(), 600);
        {
            let inner = tw.shared.inner.borrow();
            let saves = &inner.state.data.saves;
            assert_eq!(saves.len(), 600);
            assert_eq!(saves[0].name, "Save 0");
            assert_eq!(saves[k + 100].name, format!("Save {}", k + 100));
            assert!(!inner.older_in_flight);
        }
        // Saves are found by the index a station selection carries.
        assert_eq!(tw.save_at(0).map(|s| s.name), Some("Save 0".to_owned()));
        assert_eq!(tw.save_at(599).map(|s| s.name), Some("Save 599".to_owned()));
        assert_eq!(tw.save_at(600), None);
        // The owner can select a station itself; it is not told about it.
        tw.set_selection(Selection::Station(3));
        assert_eq!(
            tw.shared.inner.borrow().state.data.selection,
            Selection::Station(3)
        );
        tw.set_selection(Selection::Now);
        assert_eq!(
            tw.shared.inner.borrow().state.data.selection,
            Selection::Now
        );
        assert!(actions.borrow().is_empty());
        run_for(0.05);
        assert!(actions.borrow().is_empty());

        // The wheel zooms with or without Ctrl; set_zoom clamps and ignores nonsense.
        tw.set_zoom(1000.0);
        win.inject_wheel(wx + 500.0, line_y, 1.0, true);
        assert_close(tw.zoom(), 1150.0, 0.01);
        win.inject_wheel(wx + 500.0, line_y, -1.0, false);
        assert_close(tw.zoom(), 1000.0, 0.01);
        tw.set_zoom(700.0);
        assert_close(tw.zoom(), 700.0, 1e-3);
        tw.set_zoom(1e9);
        assert_eq!(tw.zoom(), MAX_DAY_PX as f32);
        tw.set_zoom(f32::NAN);
        tw.set_zoom(0.0);
        assert_eq!(tw.zoom(), MAX_DAY_PX as f32);
        tw.set_zoom(0.5);
        assert_eq!(tw.zoom(), MIN_DAY_PX as f32);

        // Resting on a save shows its tooltip after a moment; pressing hides it.
        tw.set_zoom(1440.0);
        tw.jump_to_end();
        run_for(0.02);
        win.inject_mouse(screen_x(597), line_y, None);
        assert_eq!(tw.shared.inner.borrow().hover, Some(Hit::Save(597)));
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
            assert_eq!(inner.state.xs().len(), 601);
            assert_eq!(inner.state.data.unsaved, 0);
            assert_eq!(inner.grow.map(|g| g.index), Some(600));
        }
        assert_close(now_x(), end_x, 0.01);
        run_for(f64::from(GROW_S) + 0.15);
        assert!(tw.shared.inner.borrow().grow.is_none());
        assert!(!tw.shared.inner.borrow().ticking);

        // A refresh at the same save updates "Now" and the note, and nothing else.
        let view_before = view();
        tw.set_unsaved(5, false);
        {
            let inner = tw.shared.inner.borrow();
            assert_eq!(inner.state.data.unsaved, 5);
            assert!(!inner.state.data.other_timelines);
            assert_eq!(inner.state.data.saves.len(), 601);
            assert_eq!(inner.state.data.selection, Selection::Station(597));
        }
        assert_eq!(view(), view_before);
        draw_frame();
        assert!(actions.borrow().is_empty());

        // At the default zoom, saves an hour apart share balls. A click on one zooms in on it
        // (the owner hears nothing of it), and the temporary zoom paints. Nothing is selected,
        // so no group is squeezed under a selected save.
        tw.set_selection(Selection::Now);
        tw.set_zoom(DEFAULT_DAY_PX as f32);
        tw.jump_to_end();
        let group_x = tw
            .shared
            .inner
            .borrow()
            .state
            .items()
            .iter()
            .rev()
            .find_map(|item| match *item {
                Item::Group { x, .. } if (0.0..f64::from(ww)).contains(&x) => Some(x),
                _ => None,
            })
            .expect("a group in view");
        let before = view();
        click(win, wx + group_x as f32, line_y);
        assert!(tw.shared.inner.borrow().state.temp.active());
        assert!(view().day_px > before.day_px);
        draw_frame();
        assert!(actions.borrow().is_empty());

        // Every zoom paints, with or without saves (it must simply not fail).
        for zoom in [MIN_DAY_PX, DEFAULT_DAY_PX, 1440.0, MAX_DAY_PX] {
            tw.set_zoom(zoom as f32);
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
