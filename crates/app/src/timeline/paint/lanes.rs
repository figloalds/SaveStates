//! Painting the lanes (spec §6.1, §6.4, §6.5; mockups B and C): the current lane's tint and the
//! separators between slots, each lane's line in its color with the curve it branches off by,
//! the dashed connector from "Você está aqui" down into the pending slot, the tags of branches
//! without a lane, each lane's name pinned to the left edge, and the scroll bar.

use savestates_ui_raven::{Painter, Rgba, text_width, tr};

use super::super::layout::Item;
use super::super::model::{
    HEADER_PX, LANE_PX, LaneData, NAME_BOX_H_PX, NAME_BOX_PAD_PX, TimelineData,
};
use super::{
    CONNECTOR_DASH, CONNECTOR_GAP, CONNECTOR_WIDTH, EDGE_PAD, GLOW_ALPHA, GLOW_WIDTH,
    GRID_LINE_ALPHA, GRID_LINE_WIDTH, HERE_RING_R, LABEL_GAP, LABEL_REACH, LINE_WIDTH, NOW_R,
    OVERDRAW, SEPARATOR, STATION_R, Scene, TAG_SIZE, dash_spans, first_save, fit_label, here_text,
    line_height, ring, with_alpha,
};
use crate::theme;

/// The current lane's tint, and the pending slot's amber one.
const CURRENT_TINT_ALPHA: f32 = 0.035;
const PENDING_TINT_ALPHA: f32 = 0.04;
/// The pending slot's dashed amber separators.
const PENDING_LINE_ALPHA: f32 = 0.45;
const PENDING_DASH: f32 = 4.0;
const PENDING_GAP: f32 = 4.0;

/// A branch curve leaves its fork ball along the parent line and lands on its lane this far to
/// the right, pulled by controls this far right of the fork, at the fork's y and the lane's.
const BRANCH_REACH: f32 = 72.0;
const BRANCH_C0: f32 = 48.0;
const BRANCH_C1: f32 = 40.0;

/// The connector into the pending slot leaves "Você está aqui"'s ring down and to the right
/// (this far right and down of its center), bends through the controls and lands in the slot
/// [`BEND_REACH`] right of the ring's center.
const BEND_START: f32 = 11.0;
const BEND_C0: (f32, f32) = (42.0, 24.0);
const BEND_C1: f32 = 40.0;
const BEND_REACH: f32 = 72.0;
/// The bend is cut into this many straight pieces to dash it.
const BEND_STEPS: usize = 24;

/// The lanes' names: at 10.5 px, cut to this width, after a dot of this radius, over a box of the
/// background at this opacity. The box is [`NAME_BOX_H_PX`] tall from the lane's top (less the
/// separator's pixel) and [`NAME_BOX_PAD_PX`] wider than the name.
pub(in crate::timeline) const LANE_NAME_SIZE: f32 = 10.5;
const LANE_NAME_MAX_W: f32 = 280.0;
const NAME_DOT_X: f32 = 9.0;
/// The middle of the dot and of the name, below the lane's top.
const NAME_MID_Y: f32 = 10.0;
const NAME_DOT_R: f32 = 3.0;
/// The pending slot's dot is a ring.
const NAME_RING_STROKE: f32 = 1.2;
const NAME_TEXT_X: f32 = 17.0;
const NAME_BOX_ALPHA: f32 = 0.82;

/// A tag: a small rounded box above its save, this far above its ring.
const TAG_PAD_X: f32 = 5.0;
const TAG_PAD_Y: f32 = 2.0;
const TAG_RADIUS: f32 = 4.0;
const TAG_ABOVE: f32 = 4.0;

/// The scroll bar, as thin as the change lists' and this far from the right edge.
const SCROLL_BAR_W: f32 = 3.0;
const SCROLL_BAR_INSET: f32 = 3.0;

/// A tag drawn over its save: its box's left edge and width, and its text.
struct ShownTag {
    save: usize,
    x: f32,
    w: f32,
    text: String,
}

/// What a lane's name box shows.
struct NameBox<'a> {
    name: &'a str,
    /// " · arquivada", in the muted color.
    suffix: Option<&'a str>,
    color: Rgba,
    bold: bool,
    /// A ring instead of a dot (the pending slot's).
    hollow: bool,
}

impl Scene<'_> {
    /// The top of the slot `lane` is drawn in.
    fn lane_top(&self, lane: usize) -> f32 {
        self.state.slot_top(self.state.slot_of_lane(lane)) as f32
    }

    /// The lane drawn as the one you are on (tinted, its name in bold): the current timeline's,
    /// or, while the next save will start a new one, the lane it continues from (mockup C).
    fn you_lane(&self) -> Option<usize> {
        self.data.current_lane.or(self.data.pending_after)
    }

    /// Whether a slot whose top is at `top` shows under the header.
    fn slot_shows(&self, top: f32) -> bool {
        top + LANE_PX as f32 > HEADER_PX as f32 && top < self.h
    }

    /// The lane you are on (see [`Scene::you_lane`]), softly tinted with its color; the pending
    /// slot, with amber.
    pub(super) fn lane_tints(&self, p: &mut Painter) {
        let lane_h = LANE_PX as f32;
        if let Some(k) = self.you_lane()
            && let Some(lane) = self.data.lanes.get(k)
        {
            let color = with_alpha(lane.color, CURRENT_TINT_ALPHA);
            p.rect(0.0, self.lane_top(k), self.w, lane_h, 0.0, color);
        }
        if let Some(slot) = self.state.pending_slot() {
            let top = self.state.slot_top(slot) as f32;
            let color = with_alpha(theme::UNSAVED, PENDING_TINT_ALPHA);
            p.rect(0.0, top, self.w, lane_h, 0.0, color);
        }
    }

    /// A thin line at every slot boundary, at the grid's alpha; dashed amber around the pending
    /// slot.
    pub(super) fn separators(&self, p: &mut Painter) {
        let pending = self.state.pending_slot();
        let solid = with_alpha(theme::BORDER, GRID_LINE_ALPHA);
        let dashed = with_alpha(theme::UNSAVED, PENDING_LINE_ALPHA);
        let shown = (HEADER_PX as f32 - 1.0)..=(self.h + 1.0);
        for slot in 0..=self.state.shown_slots() {
            let y = (self.state.slot_top(slot) as f32).round();
            if !shown.contains(&y) {
                continue;
            }
            if pending.is_some_and(|pending| slot == pending || slot == pending + 1) {
                for (x0, x1) in dash_spans(0.0, self.w, PENDING_DASH, PENDING_GAP) {
                    p.line(x0, y, x1, y, GRID_LINE_WIDTH, dashed);
                }
            } else {
                p.line(0.0, y, self.w, y, GRID_LINE_WIDTH, solid);
            }
        }
    }

    /// Each lane's line in its color, from its oldest save (or the left edge, when older saves
    /// are still to load) to its newest. A lane that branches off another curves in from the
    /// ball holding the save it branches off, landing [`BRANCH_REACH`] right of it or just
    /// before its first ball, whichever is nearer, and runs on from there. The line reaches a
    /// station that is growing in as it grows. The lane holding "Você está aqui" glows from its
    /// start to it.
    pub(super) fn lines(&self, p: &mut Painter) {
        let count = self.data.saves.len();
        let here = self.here.filter(|&i| i < count);
        let here_lane = here.and_then(|i| self.state.lane_of(i));
        for (k, lane) in self.data.lanes.iter().enumerate() {
            if lane.saves.is_empty() || lane.saves.end > count {
                continue;
            }
            let y = self.state.lane_line_y(k) as f32;
            let (first, last) = (lane.saves.start, lane.saves.end - 1);
            let curve = lane
                .fork
                .filter(|&fork| !lane.continues_left && fork < first)
                .map(|fork| {
                    let (first_x, _) = self.ball_at(first);
                    branch_curve(self.ball_at(fork), y, first_x - self.ball_radius(first))
                });
            let start = match curve {
                _ if lane.continues_left => -OVERDRAW,
                Some([.., landing]) => landing.0,
                None => self.x(first),
            };
            let before = if last > first {
                self.x(last - 1)
            } else {
                start
            };
            let end = before + (self.x(last) - before) * self.grown(last);
            if let Some(here) = here.filter(|_| here_lane == Some(k)) {
                let to = if here == last { end } else { self.x(here) };
                let glow = with_alpha(lane.color, GLOW_ALPHA);
                self.lane_path(p, curve, start, to, y, GLOW_WIDTH, glow);
            }
            self.lane_path(p, curve, start, end, y, LINE_WIDTH, lane.color);
        }
    }

    /// `curve`, then the line at `y` from `x0` to `x1` (nothing when `x1` is not right of
    /// `x0`), as far as they can show.
    #[allow(clippy::too_many_arguments)]
    fn lane_path(
        &self,
        p: &mut Painter,
        curve: Option<[(f32, f32); 4]>,
        x0: f32,
        x1: f32,
        y: f32,
        width: f32,
        color: Rgba,
    ) {
        if let Some([from, c0, c1, to]) = curve
            && to.0 >= -OVERDRAW
            && from.0 <= self.w + OVERDRAW
        {
            p.curve(from, c0, c1, to, width, color);
        }
        let (a, b) = (x0.max(-OVERDRAW), x1.min(self.w + OVERDRAW));
        if a < b {
            p.line(a, y, b, y, width, color);
        }
    }

    /// The radius of the ball holding save `i`: "Você está aqui"'s ring, a group's ball or a
    /// station.
    fn ball_radius(&self, i: usize) -> f32 {
        if Some(i) == self.here {
            return HERE_RING_R;
        }
        self.items
            .iter()
            .find_map(|item| match item {
                Item::Group {
                    members, radius, ..
                } if members.contains(&i) => Some(*radius as f32),
                _ => None,
            })
            .unwrap_or(STATION_R)
    }

    /// Where the ball holding save `i` is: its own, or its group's; where the save is when it
    /// has no ball near the view.
    fn ball_at(&self, i: usize) -> (f32, f32) {
        let x = self.items.iter().find_map(|item| match item {
            Item::Single { index, x } if *index == i => Some(*x as f32),
            Item::Group { members, x, .. } if members.contains(&i) => Some(*x as f32),
            _ => None,
        });
        (x.unwrap_or_else(|| self.x(i)), self.y(i))
    }

    /// The dashed amber connector from "You are here" to "Now": straight along their line when
    /// they share one, else bending down from the ring into "Now"'s slot.
    pub(super) fn connector(&self, p: &mut Painter) {
        let Some(here) = self.here.filter(|&i| i < self.data.saves.len()) else {
            return;
        };
        let (hx, hy, ny) = (self.x(here), self.y(here), self.now_y);
        let color = with_alpha(theme::UNSAVED, 0.85 * self.now_alpha());
        let reach = self.w + OVERDRAW;
        if hy == ny {
            let from = (hx + HERE_RING_R + 3.0).max(-OVERDRAW);
            let to = (self.now_x - NOW_R - 3.0).min(reach);
            for (x0, x1) in dash_spans(from, to, CONNECTOR_DASH, CONNECTOR_GAP) {
                p.line(x0, ny, x1, ny, CONNECTOR_WIDTH, color);
            }
            return;
        }
        let connector = pending_connector((hx, hy), (self.now_x, ny));
        let [from, .., bent] = connector.curve;
        if from.0 > reach {
            return;
        }
        let mut path = if bent.0 < -OVERDRAW {
            vec![(-OVERDRAW, ny)]
        } else {
            cubic_points(&connector.curve, BEND_STEPS)
        };
        let to = (connector.line_to.0.min(reach), ny);
        if path.last().is_some_and(|last| to.0 > last.0) {
            path.push(to);
        }
        // Each dash is short: straight from its start to its end, even across the bend.
        for dash in dash_path(&path, CONNECTOR_DASH, CONNECTOR_GAP) {
            if let (Some(a), Some(b)) = (dash.first(), dash.last()) {
                p.line(a.0, a.1, b.0, b.1, CONNECTOR_WIDTH, color);
            }
        }
    }

    /// The names of the tags on save `i`, one after the other; `None` without any.
    fn tag_text(&self, i: usize) -> Option<String> {
        let names: Vec<&str> = self
            .data
            .tags
            .iter()
            .filter(|(save, _)| *save == i)
            .map(|(_, name)| name.as_str())
            .collect();
        (!names.is_empty()).then(|| names.join(SEPARATOR))
    }

    /// The tags that fit over their saves: each save with its own ball near the view, but "Você
    /// está aqui" (its label is there), whose tag ends before the next ball of its lane (or
    /// "Agora" on it) and clear of "Você está aqui"'s label.
    fn shown_tags(&self) -> Vec<ShownTag> {
        let mut shown = Vec::new();
        if self.data.tags.is_empty() {
            return shown;
        }
        let here_half = text_width(TAG_SIZE, &here_text()) / 2.0;
        let lane_of = |item: &Item| self.state.lane_of(first_save(item));
        for (k, item) in self.items.iter().enumerate() {
            let Item::Single { index, x } = *item else {
                continue;
            };
            let cx = x as f32;
            if Some(index) == self.here || !(-LABEL_REACH..self.w + LABEL_REACH).contains(&cx) {
                continue;
            }
            let Some(text) = self.tag_text(index) else {
                continue;
            };
            let lane = self.state.lane_of(index);
            let neighbour = |j: Option<usize>| {
                j.and_then(|j| self.items.get(j))
                    .filter(|other| lane_of(other) == lane)
            };
            let next_left = match neighbour(k.checked_add(1)) {
                Some(&Item::Single { index, x }) if Some(index) == self.here => {
                    x as f32 - here_half
                }
                Some(&Item::Single { x, .. }) => x as f32 - STATION_R,
                Some(&Item::Group { x, radius, .. }) => (x - radius) as f32,
                None if lane.is_some() && lane == self.now_lane => self.now_x - NOW_R,
                None => f32::INFINITY,
            } - LABEL_GAP;
            let prev_right = match neighbour(k.checked_sub(1)) {
                Some(&Item::Single { index, x }) if Some(index) == self.here => {
                    x as f32 + here_half + LABEL_GAP
                }
                _ => f32::NEG_INFINITY,
            };
            // Not under the lane's name, which is drawn over it.
            let name_right = lane
                .and_then(|lane| self.data.lanes.get(lane))
                .map_or(f32::NEG_INFINITY, |lane| {
                    lane_name_box_width(lane_name_width(lane)) + LABEL_GAP
                });
            let prev_right = prev_right.max(name_right);
            let w = text_width(TAG_SIZE, &text) + 2.0 * TAG_PAD_X;
            if let Some(x) = tag_box(cx, w, next_left, prev_right) {
                shown.push(ShownTag {
                    save: index,
                    x,
                    w,
                    text,
                });
            }
        }
        shown
    }

    /// The names of the tags on the saves of `members`, one after the other: a group's tooltip
    /// shows them. `None` without any.
    pub(super) fn group_tags(&self, members: &std::ops::Range<usize>) -> Option<String> {
        let names: Vec<&str> = self
            .data
            .tags
            .iter()
            .filter(|(save, _)| members.contains(save))
            .map(|(_, name)| name.as_str())
            .collect();
        (!names.is_empty()).then(|| names.join(SEPARATOR))
    }

    /// The names of save `i`'s tags when they are not drawn over it: its tooltip shows them.
    pub(super) fn hidden_tags(&self, i: usize) -> Option<String> {
        let text = self.tag_text(i)?;
        let shown = self.shown_tags().iter().any(|tag| tag.save == i);
        (!shown).then_some(text)
    }

    /// The tags that fit: muted names in small raised boxes above their saves.
    pub(super) fn tags(&self, p: &mut Painter) {
        let h = line_height(TAG_SIZE) + 2.0 * TAG_PAD_Y;
        for tag in self.shown_tags() {
            let top = self.y(tag.save) - STATION_R - TAG_ABOVE - h;
            p.rect(tag.x, top, tag.w, h, TAG_RADIUS, theme::SURFACE);
            let (x, y) = (tag.x + TAG_PAD_X, top + TAG_PAD_Y);
            p.text(TAG_SIZE, x, y, &tag.text, theme::MUTED_TEXT);
        }
    }

    /// Each lane's name at its top-left, pinned to the left edge, in its color (bold for the
    /// lane you are on, with " · arquivada" when archived); the pending slot's in amber.
    pub(super) fn lane_names(&self, p: &mut Painter) {
        for (k, lane) in self.data.lanes.iter().enumerate() {
            let top = self.lane_top(k);
            if !self.slot_shows(top) {
                continue;
            }
            let (name, suffix) = lane_name(lane);
            let look = NameBox {
                name: &name,
                suffix: suffix.as_deref(),
                color: lane.color,
                bold: self.you_lane() == Some(k),
                hollow: false,
            };
            name_box(p, top, &look);
        }
        if let Some(slot) = self.state.pending_slot() {
            let top = self.state.slot_top(slot) as f32;
            if self.slot_shows(top) {
                let room = self.w - NAME_TEXT_X - EDGE_PAD;
                let measure = |s: &str| text_width(LANE_NAME_SIZE, s);
                let name = fit_label(&tr("timeline.pending"), usize::MAX, room, measure);
                let look = NameBox {
                    name: &name,
                    suffix: None,
                    color: theme::UNSAVED,
                    bold: true,
                    hollow: true,
                };
                name_box(p, top, &look);
            }
        }
    }

    /// A thin thumb at the right edge, while the lanes are taller than the widget: the view's
    /// share of them, where it is among them.
    pub(super) fn scroll_bar(&self, p: &mut Painter) {
        let content = self.state.content_height() as f32;
        let scroll = self.state.scroll_y() as f32;
        if let Some((y, h)) = scroll_thumb(self.h, content, scroll) {
            let x = self.w - SCROLL_BAR_INSET - SCROLL_BAR_W;
            p.rect(x, y, SCROLL_BAR_W, h, SCROLL_BAR_W / 2.0, theme::BORDER);
        }
    }
}

/// A name box at the top-left of the slot whose top is `top`: the background, the dot, the name
/// and its suffix.
fn name_box(p: &mut Painter, top: f32, look: &NameBox) {
    let measure = |s: &str| text_width(LANE_NAME_SIZE, s);
    let name_w = measure(look.name);
    let suffix_w = look.suffix.map_or(0.0, measure);
    let bg = with_alpha(theme::BG, NAME_BOX_ALPHA);
    let box_w = lane_name_box_width(name_w + suffix_w);
    p.rect(0.0, top + 1.0, box_w, NAME_BOX_H_PX as f32 - 1.0, 0.0, bg);
    let mid = top + NAME_MID_Y;
    if look.hollow {
        ring(p, NAME_DOT_X, mid, NAME_DOT_R, NAME_RING_STROKE, look.color);
    } else {
        p.circle(NAME_DOT_X, mid, NAME_DOT_R, look.color);
    }
    let y = mid - line_height(LANE_NAME_SIZE) / 2.0;
    if look.bold {
        p.bold_text(LANE_NAME_SIZE, NAME_TEXT_X, y, look.name, look.color);
    } else {
        p.text(LANE_NAME_SIZE, NAME_TEXT_X, y, look.name, look.color);
    }
    if let Some(suffix) = look.suffix {
        let x = NAME_TEXT_X + name_w;
        p.text(LANE_NAME_SIZE, x, y, suffix, theme::MUTED_TEXT);
    }
}

/// A lane's name as its box shows it, cut to [`LANE_NAME_MAX_W`], and " · arquivada" after it
/// when its timeline is archived.
fn lane_name(lane: &LaneData) -> (String, Option<String>) {
    let measure = |s: &str| text_width(LANE_NAME_SIZE, s);
    let name = fit_label(&lane.label, usize::MAX, LANE_NAME_MAX_W, measure);
    let suffix = lane
        .archived
        .then(|| format!("{SEPARATOR}{}", tr("timeline.archived_tag")));
    (name, suffix)
}

/// The width of each lane's name as its box shows it, in lane order: the model hit-tests the
/// name boxes with them.
pub(in crate::timeline) fn lane_label_widths(data: &TimelineData) -> Vec<f64> {
    data.lanes
        .iter()
        .map(|lane| f64::from(lane_name_width(lane)))
        .collect()
}

/// The width of `lane`'s name as its box shows it, with " · arquivada" when archived.
fn lane_name_width(lane: &LaneData) -> f32 {
    let measure = |s: &str| text_width(LANE_NAME_SIZE, s);
    let (name, suffix) = lane_name(lane);
    measure(&name) + suffix.as_deref().map_or(0.0, measure)
}

/// The width of a lane's name box for a name `label_w` wide: room for the dot before it and a
/// little after it.
pub(super) fn lane_name_box_width(label_w: f32) -> f32 {
    label_w + NAME_BOX_PAD_PX as f32
}

/// The branch curve from the ball at `from` (holding the save a lane branches off) into the
/// lane whose line is at `lane_y`: a cubic (start, two controls, end) that lands
/// [`BRANCH_REACH`] to the right, or at `before_first`, just left of the lane's first ball,
/// when that is nearer (never left of `from`). Its controls narrow with it.
pub(super) fn branch_curve(from: (f32, f32), lane_y: f32, before_first: f32) -> [(f32, f32); 4] {
    let (x, y) = from;
    let reach = (before_first - x).clamp(0.0, BRANCH_REACH);
    let k = reach / BRANCH_REACH;
    [
        (x, y),
        (x + BRANCH_C0 * k, y),
        (x + BRANCH_C1 * k, lane_y),
        (x + reach, lane_y),
    ]
}

/// The connector from "Você está aqui" at `here` to "Agora" at `now`, in another slot: a cubic
/// from the ring down into "Agora"'s line, then a line along it to "Agora"'s left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Connector {
    pub curve: [(f32, f32); 4],
    pub line_to: (f32, f32),
}

/// See [`Connector`]. The curve lands [`BEND_REACH`] right of `here`, or where the line ends
/// when "Agora" is closer than that.
pub(super) fn pending_connector(here: (f32, f32), now: (f32, f32)) -> Connector {
    let ((hx, hy), (nx, ny)) = (here, now);
    let end = nx - NOW_R - 3.0;
    let reach = (end - hx).clamp(0.0, BEND_REACH);
    let k = reach / BEND_REACH;
    Connector {
        curve: [
            (hx + BEND_START, hy + BEND_START),
            (hx + BEND_C0.0 * k, hy + BEND_C0.1),
            (hx + BEND_C1 * k, ny),
            (hx + reach, ny),
        ],
        line_to: (end, ny),
    }
}

/// Where a tag `w` wide over the ball at `cx` starts: just left of its ring, so that it runs
/// over the line toward the next ball. `None` when it would not end by `next_left` or start
/// after `prev_right`.
pub(super) fn tag_box(cx: f32, w: f32, next_left: f32, prev_right: f32) -> Option<f32> {
    let x = cx - STATION_R - 1.0;
    (x >= prev_right && x + w <= next_left).then_some(x)
}

/// The scroll bar's thumb in a widget `h` tall showing `content` px of lanes scrolled by
/// `scroll`: its top and height, the view's share of the whole. `None` when everything fits.
pub(super) fn scroll_thumb(h: f32, content: f32, scroll: f32) -> Option<(f32, f32)> {
    (h > 0.0 && content > h).then(|| (scroll * h / content, h * h / content))
}

/// `steps` + 1 points along the cubic `c`, from its start to its end.
fn cubic_points(c: &[(f32, f32); 4], steps: usize) -> Vec<(f32, f32)> {
    let steps = steps.max(1);
    (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let u = 1.0 - t;
            let (a, b, cc, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            (
                a * c[0].0 + b * c[1].0 + cc * c[2].0 + d * c[3].0,
                a * c[0].1 + b * c[1].1 + cc * c[2].1 + d * c[3].1,
            )
        })
        .collect()
}

/// The dashes along the path through `points`: `dash` long, `gap` apart, each the points it
/// passes through (a dash bends at the path's corners).
pub(super) fn dash_path(points: &[(f32, f32)], dash: f32, gap: f32) -> Vec<Vec<(f32, f32)>> {
    let mut dashes = Vec::new();
    if !(dash > 0.0 && gap >= 0.0) {
        return dashes;
    }
    let (mut on, mut left) = (true, dash);
    let mut current: Vec<(f32, f32)> = Vec::new();
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let len = (b.0 - a.0).hypot(b.1 - a.1);
        if !len.is_finite() || len <= 0.0 {
            continue;
        }
        let at = |t: f32| (a.0 + (b.0 - a.0) * t / len, a.1 + (b.1 - a.1) * t / len);
        let mut t = 0.0;
        while t < len {
            let step = left.min(len - t);
            if on {
                if current.is_empty() {
                    current.push(at(t));
                }
                current.push(at(t + step));
            }
            t += step;
            left -= step;
            if left <= 0.0 {
                if on {
                    dashes.push(std::mem::take(&mut current));
                }
                on = !on;
                left = if on { dash } else { gap };
            }
        }
    }
    if current.len() >= 2 {
        dashes.push(current);
    }
    dashes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_curve_points() {
        // From a fork ball at (100, 56) into a lane whose line is at 132.
        assert_eq!(
            branch_curve((100.0, 56.0), 132.0, 400.0),
            [(100.0, 56.0), (148.0, 56.0), (140.0, 132.0), (172.0, 132.0)]
        );
        // A first ball closer than that: the curve lands just before it, its controls narrowed
        // in proportion (36 of 72 px: half).
        assert_eq!(
            branch_curve((100.0, 56.0), 132.0, 136.0),
            [(100.0, 56.0), (124.0, 56.0), (120.0, 132.0), (136.0, 132.0)]
        );
        // A first ball at or left of the fork: straight down, never backwards.
        let [_, c0, c1, to] = branch_curve((100.0, 56.0), 132.0, 90.0);
        assert_eq!(
            (c0, c1, to),
            ((100.0, 56.0), (100.0, 132.0), (100.0, 132.0))
        );
    }

    #[test]
    fn connector_into_the_pending_slot() {
        let c = pending_connector((368.0, 56.0), (648.0, 132.0));
        assert_eq!(
            c.curve,
            [(379.0, 67.0), (410.0, 80.0), (408.0, 132.0), (440.0, 132.0)]
        );
        assert_eq!(c.line_to, (648.0 - NOW_R - 3.0, 132.0));
        // "Agora" closer than the curve's reach: the curve ends where the line would.
        let c = pending_connector((368.0, 56.0), (424.0, 132.0));
        let end = 424.0 - NOW_R - 3.0;
        assert_eq!(c.curve[3], (end, 132.0));
        assert_eq!(c.line_to, (end, 132.0));
        assert!(c.curve[1].0 < c.curve[3].0 && c.curve[2].0 < c.curve[3].0);
    }

    #[test]
    fn lane_name_box_is_the_name_plus_room_for_the_dot() {
        assert_eq!(lane_name_box_width(80.0), 104.0);
        assert_eq!(lane_name_box_width(0.0), 24.0);
    }

    #[test]
    fn a_tag_sits_between_its_ball_and_the_next() {
        // A 30 px tag over a ball at 100 starts just left of its ring.
        let x = 100.0 - STATION_R - 1.0;
        assert_eq!(tag_box(100.0, 30.0, 200.0, f32::NEG_INFINITY), Some(x));
        // It must end before what comes next, and start after what comes before.
        assert_eq!(tag_box(100.0, 30.0, x + 30.0, f32::NEG_INFINITY), Some(x));
        assert_eq!(tag_box(100.0, 30.0, x + 29.0, f32::NEG_INFINITY), None);
        assert_eq!(tag_box(100.0, 30.0, 200.0, x + 1.0), None);
    }

    #[test]
    fn the_scroll_thumb_is_the_views_share() {
        // 240 px showing 480 px of lanes: half as tall, moving half as far.
        assert_eq!(scroll_thumb(240.0, 480.0, 0.0), Some((0.0, 120.0)));
        assert_eq!(scroll_thumb(240.0, 480.0, 240.0), Some((120.0, 120.0)));
        // Everything fits: no scroll bar.
        assert_eq!(scroll_thumb(240.0, 240.0, 0.0), None);
        assert_eq!(scroll_thumb(0.0, 240.0, 0.0), None);
    }

    #[test]
    fn a_dashed_path_follows_its_corners() {
        // An L: 6 along x, then 6 down; dashes of 4 every 8.
        let path = [(0.0, 0.0), (6.0, 0.0), (6.0, 6.0)];
        let dashes = dash_path(&path, 4.0, 4.0);
        assert_eq!(
            dashes,
            [vec![(0.0, 0.0), (4.0, 0.0)], vec![(6.0, 2.0), (6.0, 6.0)],]
        );
        // A dash across the corner bends with it.
        let dashes = dash_path(&path, 8.0, 4.0);
        assert_eq!(dashes, [vec![(0.0, 0.0), (6.0, 0.0), (6.0, 2.0)]]);
        assert!(dash_path(&[(0.0, 0.0)], 4.0, 4.0).is_empty());
    }
}
