//! The timeline widget: paints a [`TimelineState`] over the day grid in a Raven custom widget
//! (see [`super::paint`]) and feeds it the widget's input (spec §4, and wave 1's §6.2).
//!
//! The state sits behind an `Rc`. The paint and event callbacks and the clock's timer hold it
//! weakly, so that it goes with the last [`TimelineWidget`] handle (when its tab closes), and
//! the clock stops then. No `RefCell` borrow is held while `on_action` runs, so the owner may
//! call back into the widget from it. Actions that come from the owner's own calls
//! (`set_data`, `set_day_px`...) are handed over on the next frame, never from inside the call.
//!
//! The owner hears of `Select`, `JumpToEnd` and `RequestOlder`. The widget acts on the others
//! itself: it repaints on `Redraw`, animates the view on `Animate`, and lists a group's saves
//! on `OpenGroupList` (picking one of them is a `Select`). `RequestOlder` goes out once, then
//! waits for `prepend_older` (or a `set_data`) before it can go out again.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use chrono::{DateTime, FixedOffset, Local, NaiveDateTime};
use savestates_core::SaveState;
use savestates_i18n::Lang;
use savestates_ui_raven::{Event, MouseButton, Painter, Widget, custom, task};

use super::axis::View;
use super::layout::{Hit, Item};
use super::model::{
    Selection, TimelineAction, TimelineData, TimelineState, handle_event, needs_older_page,
    on_escape,
};
use super::paint::{GROW_S, GroupList, LINE_Y_FRACTION, Look, Scene, ease_out};
use crate::theme;

/// The tooltip shows after the pointer rests on a station for this long.
const TOOLTIP_DELAY_S: f64 = 0.5;
/// The animations repaint at this pace.
const FRAME_S: f64 = 1.0 / 60.0;
/// A move of the view (a group click, Esc) is animated over this long, easing out.
const ANIM_S: f32 = 0.25;
/// "Agora" follows the clock at least this often, in seconds.
const CLOCK_S: f64 = 60.0;

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
        clock: Cell::new(None),
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
    custom::on_event(widget, {
        let weak = weak.clone();
        move |ev| weak.upgrade().is_some_and(|shared| shared.event(ev))
    });
    let clock = task::timer(CLOCK_S, true, move || {
        if let Some(shared) = weak.upgrade() {
            shared.tick_clock();
        }
    });
    shared.clock.set(Some(clock));
    (TimelineWidget { shared }, widget)
}

impl TimelineWidget {
    /// Shows `d`, keeping the view, with "Agora" at the current time. The first call also
    /// scrolls to the end, so that "Agora" is in view on open (at the first paint, once the
    /// widget's width is known).
    pub fn set_data(&self, d: TimelineData) {
        let shared = &self.shared;
        let older = {
            let mut inner = shared.inner.borrow_mut();
            inner.state.replace_data(d, now_local());
            inner.older_in_flight = false;
            inner.list = None;
            inner.forget_hover();
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
    /// saves every change, so "Agora" shows none until the next `set_data`. When "Agora" was in
    /// view it stays in view (which ends a temporary zoom).
    pub fn append_saved(&self, s: SaveState) {
        let shared = &self.shared;
        let start_ticking = {
            let mut guard = shared.inner.borrow_mut();
            let inner = &mut *guard;
            let now_was_visible = inner.now_on_screen();
            inner.state.data.unsaved = 0;
            inner.state.append_save(s, now_local());
            inner.list = None;
            inner.forget_hover();
            let index = inner.state.data.saves.len() - 1;
            if now_was_visible {
                inner.jump_to_end();
            }
            inner.grow = Some(Grow {
                index,
                start: Instant::now(),
            });
            inner.start_ticking()
        };
        shared.widget.invalidate();
        if start_ticking {
            shared.tick();
        }
    }

    /// Sets the zoom: the width of a day in px, clamped to the zoom range. It keeps the middle
    /// of the view in place, and ends a temporary zoom.
    pub fn set_day_px(&self, px: f64) {
        if !(px.is_finite() && px > 0.0) {
            return;
        }
        let shared = &self.shared;
        let older = {
            let mut inner = shared.inner.borrow_mut();
            inner.settle();
            inner.list = None;
            inner.forget_hover();
            let state = &mut inner.state;
            let view = &mut state.view;
            view.zoom_at(px / view.day_px, view.width / 2.0);
            state.temp.accept();
            inner.claim_older_page()
        };
        shared.widget.invalidate();
        if older {
            shared.emit_later(TimelineAction::RequestOlder);
        }
    }

    /// The zoom: the width of a day in px.
    pub fn day_px(&self) -> f64 {
        self.shared.inner.borrow().state.view.day_px
    }

    /// Esc: closes a group's list if one is open; otherwise ends a temporary zoom, going back
    /// (animated) to the view from before it. Returns true when it ended a temporary zoom.
    pub fn escape(&self) -> bool {
        let shared = &self.shared;
        let (ended, start_ticking, older) = {
            let mut inner = shared.inner.borrow_mut();
            let ended = inner.escape();
            if ended {
                inner.forget_hover();
            }
            let start_ticking = ended && inner.start_ticking();
            (ended, start_ticking, ended && inner.claim_older_page())
        };
        shared.widget.invalidate();
        if start_ticking {
            shared.tick();
        }
        if older {
            shared.emit_later(TimelineAction::RequestOlder);
        }
        ended
    }

    /// Moves "Agora" to the current time. The clock's timer calls it every minute; the owner
    /// calls it on a refresh.
    pub fn tick_clock(&self) {
        self.shared.tick_clock();
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
        {
            let mut inner = self.shared.inner.borrow_mut();
            inner.state.data.selection = selection;
            inner.list = None;
            // Pinning a save regroups the balls around it.
            inner.forget_hover();
        }
        self.shared.widget.invalidate();
    }

    /// Updates what "Agora" counts and the other-timelines note, keeping the saves, the view
    /// and the selection: for a refresh that found the project still at the same save.
    pub fn set_unsaved(&self, unsaved: usize, other_timelines: bool) {
        {
            let mut inner = self.shared.inner.borrow_mut();
            inner.state.data.unsaved = unsaved;
            inner.state.data.other_timelines = other_timelines;
        }
        self.shared.widget.invalidate();
    }

    /// Scrolls so that "Agora" sits near the right edge. This ends a temporary zoom.
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
            let shift = |m: Range<usize>| m.start + added..m.end + added;
            inner.hover = inner.hover.take().map(|hit| match hit {
                Hit::Save(i) => Hit::Save(i + added),
                Hit::Group(m) => Hit::Group(shift(m)),
                Hit::Now => Hit::Now,
            });
            if let Some(list) = &mut inner.list {
                list.members = shift(list.members.clone());
            }
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
    /// The timer that moves "Agora" with the clock.
    clock: Cell<Option<task::Timer>>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        // The widget's tab is gone: the clock has nothing left to move.
        if let Some(clock) = self.clock.take() {
            clock.cancel();
        }
    }
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

/// The view moving from `from` to the state's view.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewAnim {
    from: View,
    start: Instant,
}

struct Inner {
    state: TimelineState,
    /// The language of the dates.
    lang: Lang,
    /// `set_data` ran: until then the widget shows only its background.
    loaded: bool,
    /// The widget's size at the last paint.
    size: (f32, f32),
    /// Scroll to the end at the next paint, which knows the width.
    jump_pending: bool,
    /// `RequestOlder` went out and its page has not come back yet.
    older_in_flight: bool,
    /// The station growing in after `append_saved`.
    grow: Option<Grow>,
    /// The view on its way to the state's view, after a group click or Esc.
    anim: Option<ViewAnim>,
    /// The animations' timer chain is running.
    ticking: bool,
    /// The list of a group's saves, while it is open.
    list: Option<GroupList>,
    /// The list's row the left button went down on.
    list_press: Option<usize>,
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
        let state = TimelineState::new(data, 0.0, 0.0, to_local, now_local());
        Inner::with_state(state, lang)
    }

    fn with_state(state: TimelineState, lang: Lang) -> Inner {
        Inner {
            state,
            lang,
            loaded: false,
            size: (0.0, 0.0),
            jump_pending: false,
            older_in_flight: false,
            grow: None,
            anim: None,
            ticking: false,
            list: None,
            list_press: None,
            pressed: false,
            pointer: None,
            hover: None,
            hover_generation: 0,
            tooltip_shown: false,
        }
    }

    /// Scrolls to the end now if the width is known, else at the next paint.
    fn jump_to_end(&mut self) {
        self.anim = None;
        self.list = None;
        self.forget_hover();
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
        self.size = (width, height);
        self.state.line_y = f64::from((height * LINE_Y_FRACTION).round());
        let width = f64::from(width);
        let mut moved = false;
        if width != self.state.view.width {
            // What is at the right edge, where "Agora" usually is, stays in place.
            self.anim = None;
            self.list = None;
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

    /// Starts the animations' timer chain, unless it is running: whether the caller has to.
    fn start_ticking(&mut self) -> bool {
        !std::mem::replace(&mut self.ticking, true)
    }

    /// The view on screen while it animates; `None` once it arrived.
    fn shown_view(&self) -> Option<View> {
        let anim = self.anim?;
        let t = anim.start.elapsed().as_secs_f32() / ANIM_S;
        (t < 1.0).then(|| view_between(anim.from, self.state.view, ease_out(t)))
    }

    /// Before `ev` is acted on: a press or a wheel turn while the view animates lands it where it
    /// was going at once, so that what they act on is the view they meant (a press that turns
    /// into a drag pans from there, which ends a temporary zoom; one that doesn't keeps it).
    fn before_input(&mut self, ev: &Event) {
        if matches!(ev, Event::MouseDown { .. } | Event::Wheel { .. }) {
            self.anim = None;
        }
    }

    /// Stops the view's animation where it is on screen, for Esc to go back from there.
    fn settle(&mut self) {
        if let Some(view) = self.shown_view() {
            self.state.view = view;
        }
        self.anim = None;
    }

    /// Takes the model's actions in: animates the view, opens a group's list, and keeps the
    /// first `RequestOlder` until its page comes back. Gives what the owner has to hear of.
    fn absorb(&mut self, actions: Vec<TimelineAction>) -> Vec<TimelineAction> {
        let mut out = Vec::new();
        for action in actions {
            match action {
                TimelineAction::Redraw => {}
                TimelineAction::Animate { from } => {
                    self.anim = Some(ViewAnim {
                        from,
                        start: Instant::now(),
                    });
                }
                TimelineAction::OpenGroupList(members) => self.open_list(members),
                TimelineAction::RequestOlder => {
                    if !self.older_in_flight {
                        self.older_in_flight = true;
                        out.push(action);
                    }
                }
                TimelineAction::Select(_) | TimelineAction::JumpToEnd => out.push(action),
            }
        }
        out
    }

    /// Opens the list of the saves of the group of `members`, under its ball.
    fn open_list(&mut self, members: Range<usize>) {
        let ball = self.state.items().into_iter().find_map(|item| match item {
            Item::Group {
                members: m,
                x,
                radius,
            } if m == members => Some((x as f32, radius as f32)),
            _ => None,
        });
        if let Some((x, radius)) = ball {
            self.list = Some(GroupList::new(members, x, radius));
            self.list_press = None;
            self.tooltip_shown = false;
        }
    }

    /// What the open group list does with `ev`. `Some` (with the actions it leads to) when the
    /// list takes the event, which then goes no further: the pointer over the list, a press
    /// anywhere (one elsewhere closes the list), the wheel over it. A row picked selects its
    /// save. Leaving the widget, or turning the wheel elsewhere, closes the list.
    fn list_event(&mut self, ev: &Event) -> Option<Vec<TimelineAction>> {
        let (w, h) = self.size;
        let list = self.list.as_mut()?;
        let frame = list.frame(w, h, self.state.line_y as f32);
        match *ev {
            Event::MouseMove { x, y, .. } => {
                list.hover = list.row_at(&frame, x, y);
                frame.contains(x, y).then(Vec::new)
            }
            Event::MouseDown { x, y, button, .. } => {
                if frame.contains(x, y) {
                    let on_row = list.row_at(&frame, x, y);
                    self.list_press = on_row.filter(|_| button == MouseButton::Left);
                } else {
                    self.list = None;
                    self.list_press = None;
                }
                Some(Vec::new())
            }
            Event::MouseUp {
                x,
                y,
                button: MouseButton::Left,
            } => {
                let row = list.row_at(&frame, x, y);
                let picked = row.filter(|&row| self.list_press.take() == Some(row));
                let Some(i) = picked.and_then(|row| list.save_at(row)) else {
                    return frame.contains(x, y).then(Vec::new);
                };
                self.list = None;
                let selection = Selection::Station(i);
                self.state.data.selection = selection;
                Some(vec![TimelineAction::Select(selection)])
            }
            Event::Wheel { x, y, wy, .. } => {
                if frame.contains(x, y) {
                    list.scroll_by(wy, &frame);
                    list.hover = list.row_at(&frame, x, y);
                    return Some(Vec::new());
                }
                self.list = None;
                None
            }
            Event::Leave => {
                self.list = None;
                None
            }
            _ => None,
        }
    }

    /// Whether (`x`, `y`) is over the open group list.
    fn over_list(&self, x: f32, y: f32) -> bool {
        let (w, h) = self.size;
        self.list
            .as_ref()
            .is_some_and(|list| list.frame(w, h, self.state.line_y as f32).contains(x, y))
    }

    /// Esc: closes the group list if one is open. Otherwise, during a temporary zoom, goes back
    /// to the view from before it, animated from where the view is on screen. Returns true when
    /// it ended a temporary zoom.
    fn escape(&mut self) -> bool {
        if self.list.take().is_some() {
            return false;
        }
        if !self.state.temp.active() {
            return false;
        }
        self.settle();
        let mut ended = false;
        // `RequestOlder` is left to the caller, which asks the state again.
        for action in on_escape(&mut self.state) {
            if let TimelineAction::Animate { from } = action {
                self.anim = Some(ViewAnim {
                    from,
                    start: Instant::now(),
                });
                ended = true;
            }
        }
        ended
    }

    /// Moves "Agora" to `now`, the current local time.
    fn tick_clock(&mut self, now: NaiveDateTime) {
        self.state.set_now(now);
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

    /// Forgets what was under the pointer (and its tooltip, even one still to come) when the
    /// owner moved the view or changed the saves: it may be something else now, or gone. The
    /// pointer's next move finds out.
    fn forget_hover(&mut self) {
        self.hover = None;
        self.tooltip_shown = false;
        self.hover_generation = self.hover_generation.wrapping_add(1);
    }

    /// Drops the animation and the group list when their saves are gone from new data.
    fn forget_stale_indexes(&mut self) {
        let count = self.state.data.saves.len();
        if self.grow.is_some_and(|g| g.index >= count) {
            self.grow = None;
        }
        if self.list.as_ref().is_some_and(|l| l.members.end > count) {
            self.list = None;
        }
    }
}

impl Shared {
    fn paint(self: &Rc<Self>, p: &mut Painter, w: f32, h: f32) {
        let Ok(mut guard) = self.inner.try_borrow_mut() else {
            return;
        };
        let inner = &mut *guard;
        let moved = inner.layout(w, h);
        let older = moved && inner.claim_older_page();
        p.push_clip(0.0, 0.0, w, h);
        p.rect(0.0, 0.0, w, h, 0.0, theme::BG);
        if inner.loaded {
            // While the view animates, it is painted where it is on its way; the state keeps
            // where it goes. The pointer's hover and tooltip wait for it to arrive.
            let target = inner.state.view;
            let shown = inner.shown_view();
            if let Some(view) = shown {
                inner.state.view = view;
            }
            {
                let still = shown.is_none() && !inner.pressed;
                let look = Look {
                    lang: inner.lang,
                    hover: inner.hover.as_ref().filter(|_| still),
                    pointer: inner.pointer,
                    tooltip: inner.tooltip_shown && still,
                    grow: inner.grow.map(|g| (g.index, g.start)),
                    list: inner.list.as_ref(),
                };
                Scene::new(&inner.state, &look, w, h).paint(p);
            }
            inner.state.view = target;
        }
        p.pop_clip();
        drop(guard);
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
        inner.before_input(ev);
        let before = (
            inner.state.view,
            inner.state.temp,
            inner.hover.clone(),
            inner.tooltip_shown,
            inner.list.clone(),
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

        let actions = match inner.list_event(ev) {
            Some(actions) => actions,
            None => handle_event(&mut inner.state, ev),
        };
        let hover = inner.pointer.and_then(|(x, y)| {
            if inner.over_list(x, y) {
                return None;
            }
            inner.state.hit_at(f64::from(x), f64::from(y))
        });
        let arm = inner.set_hover(hover);
        let mut repaint = !actions.is_empty();
        let out = inner.absorb(actions);
        // A group's list hangs under its ball: it closes when the view moves.
        if inner.state.view != before.0 {
            inner.list = None;
        }
        let after = (
            inner.state.view,
            inner.state.temp,
            inner.hover.clone(),
            inner.tooltip_shown,
            inner.list.clone(),
        );
        repaint |= after != before;
        let start_ticking = inner.anim.is_some() && inner.start_ticking();
        drop(guard);

        if repaint {
            self.widget.invalidate();
        }
        if start_ticking {
            self.tick();
        }
        if let Some(generation) = arm {
            self.arm_tooltip(generation);
        }
        self.actions.emit(out);
        consumed
    }

    /// Moves "Agora" to the current time.
    fn tick_clock(&self) {
        let ticked = self
            .inner
            .try_borrow_mut()
            .map(|mut inner| inner.tick_clock(now_local()))
            .is_ok();
        if ticked {
            self.widget.invalidate();
        }
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

    /// Repaints at the animations' pace until the growing station is full size and the view
    /// arrived. One-shot timers, each arming the next: they are freed once they ran.
    fn tick(self: &Rc<Self>) {
        let shared = self.clone();
        task::timer(FRAME_S, false, move || {
            let again = match shared.inner.try_borrow_mut() {
                Ok(mut inner) => {
                    if inner
                        .grow
                        .is_some_and(|g| g.start.elapsed().as_secs_f32() >= GROW_S)
                    {
                        inner.grow = None;
                    }
                    if inner.shown_view().is_none() {
                        inner.anim = None;
                    }
                    let running = inner.grow.is_some() || inner.anim.is_some();
                    if !running {
                        inner.ticking = false;
                    }
                    running
                }
                // Busy: try again on the next tick.
                Err(_) => true,
            };
            shared.widget.invalidate();
            if again {
                shared.tick();
            }
        });
    }
}

/// The view `k` of the way (0 to 1) from `from` to `to`.
///
/// The zoom changes evenly in scale, around the one world point that is at the same screen x in
/// both views. Every point's screen x then moves straight from where it is in `from` to where
/// it is in `to`, never further: a group zoomed in on (or back out from) stays on screen all the
/// way. When the zoom hardly changes, there is no such point, and the view pans evenly.
fn view_between(from: View, to: View, k: f32) -> View {
    let k = f64::from(k);
    let width = to.width;
    let ratio = to.day_px / from.day_px;
    if !ratio.is_finite() || (ratio - 1.0).abs() < 1e-9 {
        return View {
            day_px: from.day_px + (to.day_px - from.day_px) * k,
            left: from.left + (to.left - from.left) * k,
            width,
        };
    }
    // The world point that stays put, and where it is on screen.
    let fixed = (to.left * to.day_px - from.left * from.day_px) / (to.day_px - from.day_px);
    let at = (fixed - from.left) * from.day_px;
    let day_px = from.day_px * ratio.powf(k);
    View {
        day_px,
        left: fixed - at / day_px,
        width,
    }
}

/// The computer's local time, for the model.
fn now_local() -> NaiveDateTime {
    Local::now().naive_local()
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use super::super::axis::{DEFAULT_DAY_PX, MAX_DAY_PX, MIN_DAY_PX};
    use super::super::layout::Item;
    use super::super::paint::GroupList;
    use super::*;
    use chrono::{NaiveDate, TimeDelta, TimeZone};
    use savestates_core::SaveId;
    use savestates_ui_raven::{
        Backend, Key, Window, bind_action, draw_frame, init, shutdown, wait_frame,
    };

    fn assert_close(a: f32, b: f32, tolerance: f32) {
        assert!(
            (a - b).abs() <= tolerance,
            "{a} is not within {tolerance} of {b}"
        );
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

    /// A local time on 2 November 2026 (or the day before, for a negative `day`).
    fn at(day: i64, hour: u32, minute: u32) -> NaiveDateTime {
        let date = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap() + TimeDelta::days(day);
        date.and_hms_opt(hour, minute, 0).unwrap()
    }

    /// "Save {n}" committed at the local time `local` (UTC-3).
    fn save_local(n: usize, local: NaiveDateTime) -> SaveState {
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

    /// The widget's state, 1,000 px wide, without Raven: saves at these local times and a
    /// clock at 12:00 on 2 November 2026.
    fn inner_with(times: &[NaiveDateTime]) -> Inner {
        let data = TimelineData {
            saves: times
                .iter()
                .enumerate()
                .map(|(n, &t)| save_local(n, t))
                .collect(),
            more_available: false,
            unsaved: 2,
            selection: Selection::Now,
            other_timelines: false,
        };
        let own_offset = |t: DateTime<FixedOffset>| t.naive_local();
        let state = TimelineState::new(data, 1000.0, 100.0, own_offset, at(0, 12, 0));
        let mut inner = Inner::with_state(state, Lang::PtBr);
        inner.loaded = true;
        inner
    }

    /// What a click at screen x `x` on the line does, through the model and the widget.
    fn click_at(inner: &mut Inner, x: f64) -> Vec<TimelineAction> {
        let (x, y) = (x as f32, inner.state.line_y as f32);
        let press = Event::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            clicks: 1,
        };
        let release = Event::MouseUp {
            x,
            y,
            button: MouseButton::Left,
        };
        let mut out = Vec::new();
        for ev in [press, release] {
            let actions = handle_event(&mut inner.state, &ev);
            out.extend(inner.absorb(actions));
        }
        out
    }

    /// The screen x of the group ball holding exactly `members`.
    fn group_x(inner: &Inner, members: Range<usize>) -> f64 {
        inner
            .state
            .items()
            .iter()
            .find_map(|item| match item {
                Item::Group { members: m, x, .. } if *m == members => Some(*x),
                _ => None,
            })
            .expect("the group")
    }

    /// Where world x `x` is on screen at each of 101 even steps of the animation from `from`
    /// to `to`.
    fn path(from: View, to: View, x: f64) -> Vec<f64> {
        (0..=100)
            .map(|i| view_between(from, to, i as f32 / 100.0).world_to_screen(x))
            .collect()
    }

    /// Checks that `xs` goes from `start` to `end` without ever turning back.
    fn assert_moves_straight(xs: &[f64], start: f64, end: f64, context: &str) {
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-6 * (1.0 + b.abs());
        assert!(
            close(xs[0], start),
            "{context}: starts at {} not {start}",
            xs[0]
        );
        let last = xs[xs.len() - 1];
        assert!(close(last, end), "{context}: ends at {last} not {end}");
        for pair in xs.windows(2) {
            let step = pair[1] - pair[0];
            assert!(
                step * (end - start) >= -1e-6,
                "{context}: turns back from {} to {}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn view_animation_keeps_the_group_on_its_way() {
        // About a week at 140 px a day, 1,100 px wide, with today on the right.
        let from = View {
            day_px: 140.0,
            left: -6.5,
            width: 1100.0,
        };
        let minute = 1.0 / 1440.0;
        // The afternoon's group yesterday (13:05 to 17:40), and saves three minutes apart
        // yesterday at 10:00, which a click zooms all the way in on (140 to 57,600 px a day).
        let afternoon = (-1.0 + 785.0 * minute, -1.0 + 1060.0 * minute);
        let minutes = (-1.0 + 600.0 * minute, -1.0 + 603.0 * minute);
        for (name, (first, last)) in [("afternoon", afternoon), ("minutes", minutes)] {
            let zoomed = from.fit(first, last, 0.6);
            for (way, start, end) in [("click", from, zoomed), ("Esc", zoomed, from)] {
                for x in [first, last] {
                    let xs = path(start, end, x);
                    let context = format!("{name}, {way}, save at {x}");
                    let (a, b) = (start.world_to_screen(x), end.world_to_screen(x));
                    // On screen at both ends, so on screen all the way.
                    assert!((0.0..=1100.0).contains(&a) && (0.0..=1100.0).contains(&b));
                    assert_moves_straight(&xs, a, b, &context);
                }
            }
        }
        // The zoom changes evenly in scale.
        let zoomed = from.fit(minutes.0, minutes.1, 0.6);
        let middle = view_between(from, zoomed, 0.5);
        assert!((middle.day_px - (140.0_f64 * MAX_DAY_PX).sqrt()).abs() < 1e-6);
        assert_eq!(middle.width, 1100.0);
        // Without a change of zoom (a pan), the view moves evenly.
        let panned = View {
            left: from.left + 2.0,
            ..from
        };
        let middle = view_between(from, panned, 0.5);
        assert_eq!(middle.day_px, 140.0);
        assert!((middle.left - (from.left + 1.0)).abs() < 1e-12);
    }

    #[test]
    fn a_press_during_the_animation_lands_on_its_target() {
        let mut inner = inner_with(&[
            at(-1, 10, 0),
            at(-1, 10, 1),
            at(-1, 10, 30),
            at(-1, 10, 31),
            at(-1, 11, 0),
            at(0, 6, 0),
        ]);
        let x = group_x(&inner, 0..5);
        click_at(&mut inner, x);
        let zoomed = inner.state.view;
        assert!(inner.anim.is_some());
        // Pressing stops the animation where it was going, and keeps the temporary zoom: only
        // a drag that moves pans (and ends it).
        let press = Event::MouseDown {
            x: 500.0,
            y: 100.0,
            button: MouseButton::Left,
            clicks: 1,
        };
        inner.before_input(&press);
        assert!(inner.anim.is_none());
        assert_eq!(inner.state.view, zoomed);
        assert!(inner.state.temp.active());
        // The wheel too; a move changes nothing.
        inner.anim = Some(ViewAnim {
            from: zoomed,
            start: Instant::now(),
        });
        let moved = Event::MouseMove {
            x: 501.0,
            y: 100.0,
            dx: 1.0,
            dy: 0.0,
        };
        inner.before_input(&moved);
        assert!(inner.anim.is_some());
        let wheel = Event::Wheel {
            x: 500.0,
            y: 100.0,
            wx: 0.0,
            wy: 1.0,
            ctrl: false,
            shift: false,
        };
        inner.before_input(&wheel);
        assert!(inner.anim.is_none());
        assert_eq!(inner.state.view, zoomed);
    }

    #[test]
    fn escape_ends_temp_zoom_once() {
        // Five saves within an hour yesterday share a ball; "you are here" is at 06:00 today.
        let mut inner = inner_with(&[
            at(-1, 10, 0),
            at(-1, 10, 1),
            at(-1, 10, 30),
            at(-1, 10, 31),
            at(-1, 11, 0),
            at(0, 6, 0),
        ]);
        let before = inner.state.view;
        let x = group_x(&inner, 0..5);
        // A click on the group zooms in on it, animated; the owner hears nothing of it.
        assert!(click_at(&mut inner, x).is_empty());
        assert!(inner.state.temp.active());
        let zoomed = inner.state.view;
        assert_eq!(inner.anim.map(|a| a.from), Some(before));
        // The animation runs its course (the widget's timers end it).
        inner.anim = None;

        // Esc goes back to the view from before, animated from the zoomed one...
        assert!(inner.escape());
        assert_eq!(inner.state.view, before);
        assert!(!inner.state.temp.active());
        assert_eq!(inner.anim.map(|a| a.from), Some(zoomed));
        // ...once: there is nothing more to go back to.
        let anim = inner.anim;
        assert!(!inner.escape());
        assert_eq!(inner.state.view, before);
        assert_eq!(inner.anim, anim);

        // With a group's list open, Esc closes the list and keeps the temporary zoom.
        click_at(&mut inner, x);
        assert!(inner.state.temp.active());
        inner.list = Some(GroupList::new(0..2, 500.0, 9.0));
        assert!(!inner.escape());
        assert!(inner.list.is_none());
        assert!(inner.state.temp.active());
        assert!(inner.escape());
        assert!(!inner.state.temp.active());
    }

    #[test]
    fn group_list_picks_a_save_and_closes() {
        // Six saves in the same second yesterday, between two others.
        let mut times = vec![at(-1, 9, 0)];
        times.extend([at(-1, 16, 20); 6]);
        times.push(at(-1, 17, 20));
        let mut inner = inner_with(&times);
        inner.size = (1000.0, 240.0);
        let press = |x: f32, y: f32| Event::MouseDown {
            x,
            y,
            button: MouseButton::Left,
            clicks: 1,
        };
        let release = |x: f32, y: f32| Event::MouseUp {
            x,
            y,
            button: MouseButton::Left,
        };
        let open = |inner: &mut Inner| {
            let x = group_x(inner, 1..7);
            // The group can't split: a click lists its saves, under its ball.
            assert!(click_at(inner, x).is_empty());
            let list = inner.list.clone().expect("the list is open");
            assert_eq!(list.members, 1..7);
            list.frame(1000.0, 240.0, inner.state.line_y as f32)
        };

        // Picking the second row selects the second newest save, and closes the list.
        let frame = open(&mut inner);
        let (x, y) = (frame.x + 20.0, frame.y + 4.0 + 1.5 * 20.0);
        assert_eq!(inner.list_event(&press(x, y)), Some(Vec::new()));
        let picked = inner.list_event(&release(x, y));
        assert_eq!(
            picked,
            Some(vec![TimelineAction::Select(Selection::Station(5))])
        );
        assert_eq!(inner.state.data.selection, Selection::Station(5));
        assert!(inner.list.is_none());

        // A press elsewhere closes it, and does nothing else.
        inner.state.data.selection = Selection::Now;
        let frame = open(&mut inner);
        let elsewhere = (frame.x + frame.w + 50.0, frame.y);
        assert_eq!(
            inner.list_event(&press(elsewhere.0, elsewhere.1)),
            Some(Vec::new())
        );
        assert!(inner.list.is_none());
        assert_eq!(inner.state.data.selection, Selection::Now);

        // Leaving the widget closes it; so does the wheel elsewhere, which then zooms.
        open(&mut inner);
        assert_eq!(inner.list_event(&Event::Leave), None);
        assert!(inner.list.is_none());
        let frame = open(&mut inner);
        let wheel = |x: f32, y: f32| Event::Wheel {
            x,
            y,
            wx: 0.0,
            wy: -1.0,
            ctrl: false,
            shift: false,
        };
        // Over the list, the wheel scrolls it (five of the six rows fit under the ball).
        assert_eq!(frame.shown, 5);
        assert_eq!(
            inner.list_event(&wheel(frame.x + 5.0, frame.y + 5.0)),
            Some(Vec::new())
        );
        assert_eq!(inner.list.as_ref().map(|list| list.scroll), Some(1));
        assert_eq!(inner.list_event(&wheel(elsewhere.0, elsewhere.1)), None);
        assert!(inner.list.is_none());
    }

    #[test]
    fn clock_tick_moves_agora() {
        // "You are here" at 06:00; at 12:00 "Agora" keeps its minimum gap (6 hours is 35 px).
        let mut inner = inner_with(&[at(-1, 9, 0), at(0, 6, 0)]);
        let here_x = |inner: &Inner| inner.state.view.world_to_screen(inner.state.xs()[1]);
        let view = inner.state.view;
        let gap = inner.state.now_screen_x() - here_x(&inner);
        assert!((gap - 56.0).abs() < 1e-6, "{gap}");
        // At 18:00 it sits at its own time, 12 hours (70 px) after it. The view stays.
        inner.tick_clock(at(0, 18, 0));
        let gap = inner.state.now_screen_x() - here_x(&inner);
        assert!((gap - 70.0).abs() < 1e-6, "{gap}");
        assert_eq!(inner.state.view, view);
        // A day later, a day further.
        inner.tick_clock(at(1, 18, 0));
        let gap = inner.state.now_screen_x() - here_x(&inner);
        assert!((gap - 210.0).abs() < 1e-6, "{gap}");
    }

    /// Everything that needs Raven is in this one test: Raven's UI thread is the thread that
    /// called `init`. Input goes through Raven's real event path on the NULL backend.
    #[test]
    fn the_widget_on_the_null_backend() {
        use TimelineAction::{JumpToEnd, RequestOlder, Select};

        init(Backend::Null).expect("init on the NULL backend");
        // A text input above the timeline, like peek's file filter in a project tab.
        let win = Window::from_xml(
            r#"<Window Title="t" Width="1000" Height="270"><VBox><TextInput Name="filter"/><Overlay Name="host" Slot.Fill="1"/></VBox></Window>"#,
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
                    let _ = tw.day_px();
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
        tw.set_day_px(1440.0);
        tw.jump_to_end();
        assert_eq!(tw.day_px(), 1440.0);
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

        // The wheel zooms with or without Ctrl; set_day_px clamps and ignores nonsense.
        tw.set_day_px(1000.0);
        win.inject_wheel(wx + 500.0, line_y, 1.0, true);
        assert!((tw.day_px() - 1150.0).abs() < 1e-6);
        win.inject_wheel(wx + 500.0, line_y, -1.0, false);
        assert!((tw.day_px() - 1000.0).abs() < 1e-6);
        tw.set_day_px(700.0);
        assert!((tw.day_px() - 700.0).abs() < 1e-9);
        tw.set_day_px(1e9);
        assert_eq!(tw.day_px(), MAX_DAY_PX);
        tw.set_day_px(f64::NAN);
        tw.set_day_px(0.0);
        tw.set_day_px(-3.0);
        assert_eq!(tw.day_px(), MAX_DAY_PX);
        tw.set_day_px(0.5);
        assert_eq!(tw.day_px(), MIN_DAY_PX);

        // Resting on a save shows its tooltip after a moment; pressing hides it.
        tw.set_day_px(1440.0);
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
        // The owner moving the view forgets what was under the pointer, and its tooltip: it
        // may be something else now.
        win.inject_mouse(screen_x(596), line_y, None);
        run_for(TOOLTIP_DELAY_S + 0.2);
        assert_eq!(tw.shared.inner.borrow().hover, Some(Hit::Save(596)));
        assert!(tw.shared.inner.borrow().tooltip_shown);
        tw.jump_to_end();
        assert_eq!(tw.shared.inner.borrow().hover, None);
        assert!(!tw.shared.inner.borrow().tooltip_shown);

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
        tw.set_day_px(DEFAULT_DAY_PX);
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
        // Esc reaches the timeline through a window shortcut, as the app routes it. While a text
        // input has the keyboard, Esc is the text input's...
        let escapes: Rc<Cell<usize>> = Rc::default();
        win.register_shortcut(Key::Escape, false, false, "test.escape");
        bind_action("test.escape", {
            let (tw, escapes) = (tw.clone(), escapes.clone());
            move |_| {
                escapes.set(escapes.get() + 1);
                tw.escape();
            }
        });
        win.find("filter").expect("filter").focus();
        draw_frame();
        win.inject_key(Key::Escape, false);
        assert_eq!(escapes.get(), 0);
        let before = view();
        click(win, wx + group_x as f32, line_y);
        assert!(tw.shared.inner.borrow().state.temp.active());
        assert!(view().day_px > before.day_px);
        // The view animates there, then stops ticking.
        assert!(tw.shared.inner.borrow().anim.is_some());
        draw_frame();
        run_for(f64::from(ANIM_S) + 0.15);
        assert!(tw.shared.inner.borrow().anim.is_none());
        assert!(!tw.shared.inner.borrow().ticking);
        assert!(actions.borrow().is_empty());

        // ...but a press on the timeline (the group click) took the keyboard from it: Esc goes
        // back, animated, once; nothing reaches the owner.
        let zoomed = view();
        win.inject_key(Key::Escape, false);
        assert_eq!(escapes.get(), 1);
        assert!(!tw.shared.inner.borrow().state.temp.active());
        assert_eq!(view(), before);
        assert_eq!(tw.shared.inner.borrow().anim.map(|a| a.from), Some(zoomed));
        assert!(!tw.escape());
        run_for(f64::from(ANIM_S) + 0.15);
        assert!(tw.shared.inner.borrow().anim.is_none());
        assert!(actions.borrow().is_empty());

        // The clock moves "Agora" and nothing else.
        let view_before = view();
        tw.tick_clock();
        assert_eq!(view(), view_before);
        draw_frame();

        // Every zoom paints, with or without saves (it must simply not fail).
        for zoom in [MIN_DAY_PX, DEFAULT_DAY_PX, 1440.0, MAX_DAY_PX] {
            tw.set_day_px(zoom);
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
