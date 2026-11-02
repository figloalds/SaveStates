//! Pure layout of the timeline's balls: which saves merge into a group ball at the current
//! zoom, where each ball sits on screen, and what the pointer is on. Nothing here draws.
//!
//! Saves come as world xs (see [`super::axis`]), oldest first and never decreasing. Walking
//! them in that order, a save joins the open group while it is closer than [`GROUP_JOIN_PX`]
//! to the group's *first* member, so a long chain of saves becomes several neighbouring groups
//! instead of one huge one. Pinned saves ("Você está aqui" and the selected save) always keep
//! their own ball: they split the walk. Last, neighbouring balls are moved apart: balls that
//! aren't pinned never overlap each other, nor a pinned ball as it is painted (with its ring or
//! halo) when there is room. Pinned balls never move, so the others move away from them; only
//! a ball squeezed between two pinned balls close together may overlap them, and the painter
//! draws pinned balls on top.

use std::ops::Range;

use super::axis::{Days, View};

/// A single save's ball.
pub const BALL_RADIUS_PX: f64 = 7.0;
/// A save joins the open group while it is closer than this to the group's first member: about
/// two ball widths.
pub const GROUP_JOIN_PX: f64 = 28.0;
/// Group balls grow with their count up to this radius.
pub const GROUP_MAX_RADIUS_PX: f64 = 13.0;
/// How close (in screen pixels) the pointer must be to a single save or "Agora" to hit it. A
/// group reaches at least as far.
pub const HIT_RADIUS_PX: f64 = 12.0;
/// A pinned ball ("Você está aqui", or the selected save) is painted larger than a single save,
/// with a ring or a halo: the room it takes from its neighbours, and how far it reaches.
pub const PINNED_RADIUS_PX: f64 = 15.0;

/// The space kept between neighbouring balls.
const BALL_GAP_PX: f64 = 2.0;
/// Separation ignores a push smaller than this: rounding, not an overlap.
const ROUNDING_PX: f64 = 1e-9;
/// A gap between two saves wide enough that the balls before it can't move those after it, nor
/// the reverse (see [`layout_range`]). Separating (see [`separate`]) moves a ball at most 30 px
/// right and less than 44 px left of where grouping put it, and two balls need at most 30 px
/// between their centers (a group next to a pinned ball): 30 + 44 + 30 = 104 px, and this is
/// 112.
const RESYNC_GAP_PX: f64 = 4.0 * GROUP_JOIN_PX;

/// One ball on the line. `x` is its screen x.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// The save at this index, on its own.
    Single { index: usize, x: f64 },
    /// The saves at these indexes, merged into one ball of this radius.
    Group {
        members: Range<usize>,
        x: f64,
        radius: f64,
    },
}

impl Item {
    /// The ball's screen x.
    pub(crate) fn x(&self) -> f64 {
        match *self {
            Item::Single { x, .. } | Item::Group { x, .. } => x,
        }
    }

    fn x_mut(&mut self) -> &mut f64 {
        match self {
            Item::Single { x, .. } | Item::Group { x, .. } => x,
        }
    }

    fn radius(&self) -> f64 {
        match *self {
            Item::Single { .. } => BALL_RADIUS_PX,
            Item::Group { radius, .. } => radius,
        }
    }
}

/// What the pointer is on.
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    /// The save at this index.
    Save(usize),
    /// The group holding these saves.
    Group(Range<usize>),
    /// "Agora".
    Now,
}

/// The radius of a group ball of `count` saves: it grows gently with the count, capped.
pub fn group_radius(count: usize) -> f64 {
    let count = count.max(1) as f64;
    (BALL_RADIUS_PX + 2.0 * count.log2()).min(GROUP_MAX_RADIUS_PX)
}

/// The saves near enough to the view to lay out: those on screen plus one screen width of
/// margin on each side. `xs` must not decrease.
pub fn visible_range(xs: &[Days], view: &View) -> Range<usize> {
    let from = view.screen_to_world(-view.width);
    let to = view.screen_to_world(2.0 * view.width);
    let start = xs.partition_point(|&x| x < from);
    let end = xs.partition_point(|&x| x <= to).max(start);
    start..end
}

/// The saves to lay out for `view`: [`visible_range`], widened on each side to the nearest
/// point where the layout starts afresh whatever lies beyond it: the first or last save, or a
/// gap of [`RESYNC_GAP_PX`] between two saves. (A pinned save is not such a point: the balls on
/// either side of it keep apart from each other.) Laid out from there, the balls on screen are
/// those of the whole history, so they don't change as the edges of the range move while
/// panning.
pub fn layout_range(view: &View, xs: &[Days]) -> Range<usize> {
    let visible = visible_range(xs, view);
    if visible.is_empty() {
        return visible;
    }
    let gap = |i: usize| view.world_to_screen(xs[i]) - view.world_to_screen(xs[i - 1]);
    let mut start = visible.start;
    while start > 0 && gap(start) < RESYNC_GAP_PX {
        start -= 1;
    }
    let mut end = visible.end;
    while end < xs.len() && gap(end) < RESYNC_GAP_PX {
        end += 1;
    }
    start..end
}

/// The balls for the saves in `range`, in index order:
/// 1. Walking in index order, a save joins the open group while its screen x is less than
///    [`GROUP_JOIN_PX`] after the group's first member's.
/// 2. Pinned indexes never join a group: each closes the open group and keeps its own ball.
/// 3. A group of one is an [`Item::Single`].
/// 4. A group sits at the middle of its members' screen span.
/// 5. Balls are moved apart, keeping their order, so that no two balls that aren't pinned
///    overlap, with 2 px between them. Pinned balls never move, and take [`PINNED_RADIUS_PX`]:
///    a ball closer to one than its radius + [`PINNED_RADIUS_PX`] + 2 px moves away from it, to
///    that distance, when there is room. Only pinned balls may overlap another ball: one
///    squeezed between two pinned balls too close for it (see [`separate`]).
pub fn layout(view: &View, xs: &[Days], range: Range<usize>, pinned: &[usize]) -> Vec<Item> {
    let end = range.end.min(xs.len());
    let range = range.start.min(end)..end;
    let screen_x = |i: usize| view.world_to_screen(xs[i]);
    let mut items = Vec::new();
    // The open group: its first member and that member's screen x.
    let mut open: Option<(usize, f64)> = None;
    for i in range.clone() {
        let x = screen_x(i);
        if pinned.contains(&i) {
            close_group(&mut items, open.take(), i, screen_x);
            items.push(Item::Single { index: i, x });
            continue;
        }
        match open {
            Some((_, first_x)) if x - first_x < GROUP_JOIN_PX => {}
            _ => {
                close_group(&mut items, open.take(), i, screen_x);
                open = Some((i, x));
            }
        }
    }
    close_group(&mut items, open, range.end, screen_x);
    separate(&mut items, pinned);
    items
}

/// Ends the open group, whose members run from `open`'s first member up to `end`, and adds its
/// ball.
fn close_group(
    items: &mut Vec<Item>,
    open: Option<(usize, f64)>,
    end: usize,
    screen_x: impl Fn(usize) -> f64,
) {
    let Some((first, first_x)) = open else {
        return;
    };
    let count = end - first;
    items.push(if count == 1 {
        Item::Single {
            index: first,
            x: first_x,
        }
    } else {
        Item::Group {
            members: first..end,
            x: first_x.midpoint(screen_x(end - 1)),
            radius: group_radius(count),
        }
    });
}

/// Moves the balls (in index order) apart, so that two balls are "apart" when their centers
/// are at least both radii + [`BALL_GAP_PX`] from each other, a pinned ball counting as
/// [`PINNED_RADIUS_PX`] wide. Pinned balls never move. With at most two pinned balls (the model
/// has "Você está aqui" and the selected save):
/// - the balls stay in index order: x never decreases;
/// - every two balls that aren't pinned are apart;
/// - a ball that isn't pinned is apart from a pinned one whenever there is room. When two
///   pinned balls are too close for the balls between them (or for each other), those balls
///   stay between the two, overlapping them, and the painter draws pinned balls on top.
///
/// Three sweeps:
/// 1. Forward: each ball that overlaps the one before it moves right.
/// 2. Backward: each ball that overlaps the one after it moves left. It also keeps apart from
///    the first ball past a pinned one after it. A ball after a pinned one never moves left of
///    its floor: the pinned ball's x, plus room for the balls between them. After sweep 1 this
///    sweep only moves balls held back by a pinned ball on their right.
/// 3. Forward again, for the balls past the last pinned ball: they keep apart from a ball
///    squeezed between two pinned balls before them.
///
/// Balls that aren't pinned may drift from their time this way, by at most 30 px to the right
/// and less than 44 px to the left.
fn separate(items: &mut [Item], pinned: &[usize]) {
    let is_pinned =
        |item: &Item| matches!(*item, Item::Single { index, .. } if pinned.contains(&index));
    let room = |item: &Item| {
        if is_pinned(item) {
            PINNED_RADIUS_PX
        } else {
            item.radius()
        }
    };
    let spacing = |a: &Item, b: &Item| room(a) + room(b) + BALL_GAP_PX;
    let count = items.len();

    for k in 1..count {
        let least = items[k - 1].x() + spacing(&items[k - 1], &items[k]);
        if !is_pinned(&items[k]) && items[k].x() < least {
            *items[k].x_mut() = least;
        }
    }

    // The floor of each ball that isn't pinned: the pinned ball before it, plus room for the
    // balls between them. Room for them all exists, as their saves are 28 px apart or more.
    let mut floor = vec![f64::NEG_INFINITY; count];
    let mut pinned_x = f64::NEG_INFINITY;
    let mut previous: Option<usize> = None;
    for k in 0..count {
        if is_pinned(&items[k]) {
            pinned_x = items[k].x();
            previous = None;
            continue;
        }
        floor[k] = match previous {
            Some(j) => floor[j] + spacing(&items[j], &items[k]),
            None => pinned_x,
        };
        previous = Some(k);
    }

    for k in (0..count.saturating_sub(1)).rev() {
        if is_pinned(&items[k]) {
            continue;
        }
        let next = &items[k + 1];
        let mut most = items[k].x().min(next.x() - spacing(&items[k], next));
        if is_pinned(next)
            && let Some(past) = items[k + 1..].iter().find(|item| !is_pinned(item))
        {
            most = most.min(past.x() - spacing(&items[k], past));
        }
        *items[k].x_mut() = most.max(floor[k]);
    }

    let Some(last_pinned) = items.iter().rposition(is_pinned) else {
        return;
    };
    let mut previous = items[..last_pinned]
        .iter()
        .rposition(|item| !is_pinned(item));
    for k in last_pinned + 1..count {
        if let Some(j) = previous {
            let least = items[j].x() + spacing(&items[j], &items[k]);
            if items[k].x() < least - ROUNDING_PX {
                *items[k].x_mut() = least;
            }
        }
        previous = Some(k);
    }
}

/// What the pointer at (`x`, `y`) is on. Every ball sits at `line_y`.
///
/// The `pinned` saves' balls are painted on top of the others, [`PINNED_RADIUS_PX`] wide: within
/// that of one, the nearest such ball wins. Otherwise it is the nearest ball within its reach:
/// a single save and "Agora" (at screen x `now_x`) reach [`HIT_RADIUS_PX`]; a group reaches its
/// radius, but never less than a single save.
pub fn hit_test(
    items: &[Item],
    pinned: &[usize],
    now_x: f64,
    x: f64,
    y: f64,
    line_y: f64,
) -> Option<Hit> {
    let dy = y - line_y;
    let distance = |ball_x: f64| (ball_x - x).hypot(dy);
    let on_top = items
        .iter()
        .filter_map(|item| match *item {
            Item::Single { index, x } if pinned.contains(&index) => Some((index, distance(x))),
            _ => None,
        })
        .filter(|&(_, d)| d <= PINNED_RADIUS_PX)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((index, _)) = on_top {
        return Some(Hit::Save(index));
    }
    let mut best: Option<(Hit, f64)> = None;
    let mut consider = |hit: Hit, ball_x: f64, reach: f64| {
        let d = distance(ball_x);
        if d <= reach && best.as_ref().is_none_or(|(_, nearest)| d < *nearest) {
            best = Some((hit, d));
        }
    };
    for item in items {
        let reach = item.radius().max(HIT_RADIUS_PX);
        match item {
            Item::Single { index, x } => consider(Hit::Save(*index), *x, reach),
            Item::Group { members, x, .. } => consider(Hit::Group(members.clone()), *x, reach),
        }
    }
    consider(Hit::Now, now_x, HIT_RADIUS_PX);
    best.map(|(hit, _)| hit)
}

/// Whether the saves in `members`, laid out on their own at `view`, take more than one ball.
pub fn splits_at(view: &View, xs: &[Days], members: Range<usize>) -> bool {
    layout(view, xs, members, &[]).len() > 1
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::timeline::axis::{DEFAULT_DAY_PX, MAX_DAY_PX, MIN_DAY_PX};

    /// A power of two, so that whole screen pixels give exact world xs and back.
    const DAY_PX: f64 = 64.0;
    const LINE_Y: f64 = 100.0;

    fn view() -> View {
        View {
            day_px: DAY_PX,
            left: 0.0,
            width: 1000.0,
        }
    }

    /// The world xs of saves at these screen xs in [`view`].
    fn at_px(px: impl IntoIterator<Item = f64>) -> Vec<Days> {
        px.into_iter().map(|p| p / DAY_PX).collect()
    }

    /// `count` saves `gap` px apart in [`view`], the first at 100 px.
    fn evenly(count: usize, gap: f64) -> Vec<Days> {
        at_px((0..count).map(|i| 100.0 + i as f64 * gap))
    }

    fn single(index: usize, x: f64) -> Item {
        Item::Single { index, x }
    }

    fn members(item: &Item) -> Range<usize> {
        match item {
            Item::Single { index, .. } => *index..*index + 1,
            Item::Group { members, .. } => members.clone(),
        }
    }

    /// The items whose ball is on screen.
    fn on_screen(items: &[Item], view: &View) -> Vec<Item> {
        items
            .iter()
            .filter(|item| (0.0..view.width).contains(&item.x()))
            .cloned()
            .collect()
    }

    /// Dense, uneven world xs from a fixed seed: runs of saves seconds to an hour apart (some
    /// at the same time), with gaps of up to three days between runs.
    fn random_xs(seed: u64, count: usize) -> Vec<Days> {
        let mut state = seed;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 11) as f64 / (1_u64 << 53) as f64
        };
        let mut x = 0.0;
        (0..count)
            .map(|_| {
                let (kind, size) = (next(), next());
                x += if kind < 0.05 {
                    0.0
                } else if kind < 0.9 {
                    size.powi(3) / 24.0
                } else {
                    3.0 * size
                };
                x
            })
            .collect()
    }

    /// Whether `a` and `b`, neighbours, have at least 2 px between their circles.
    fn apart(a: &Item, b: &Item) -> bool {
        b.x() - a.x() >= a.radius() + b.radius() + 2.0 - 1e-9
    }

    #[test]
    fn separate_saves_stay_single() {
        // 28 px apart is far enough: a save joins only when it is closer than that.
        let xs = at_px([100.0, 140.0, 200.0, 228.0, 400.0]);
        assert_eq!(
            layout(&view(), &xs, 0..5, &[]),
            vec![
                single(0, 100.0),
                single(1, 140.0),
                single(2, 200.0),
                single(3, 228.0),
                single(4, 400.0)
            ]
        );
        // Only the range is laid out.
        assert_eq!(
            layout(&view(), &xs, 1..3, &[]),
            vec![single(1, 140.0), single(2, 200.0)]
        );
        assert!(layout(&view(), &xs, 2..2, &[]).is_empty());
    }

    #[test]
    fn close_saves_group_from_first_member() {
        // Five saves 6 px apart span 24 px: one group.
        let xs = evenly(5, 6.0);
        assert_eq!(
            layout(&view(), &xs, 0..5, &[]),
            vec![Item::Group {
                members: 0..5,
                x: 112.0,
                radius: group_radius(5)
            }]
        );

        // Five saves 10 px apart: the fourth is 30 px from the first, so it starts the next group.
        let xs = evenly(5, 10.0);
        let groups: Vec<Range<usize>> = layout(&view(), &xs, 0..5, &[])
            .iter()
            .map(members)
            .collect();
        assert_eq!(groups, [0..3, 3..5]);

        // A long chain becomes neighbouring groups, never one huge one.
        let xs = evenly(40, 20.0);
        let items = layout(&view(), &xs, 0..40, &[]);
        assert_eq!(items.len(), 20);
        for (k, item) in items.iter().enumerate() {
            let Item::Group { members, .. } = item else {
                panic!("not a group: {item:?}");
            };
            assert_eq!(*members, 2 * k..2 * k + 2);
            let first = view().world_to_screen(xs[members.start]);
            let last = view().world_to_screen(xs[members.end - 1]);
            assert!(last - first < GROUP_JOIN_PX);
        }
    }

    #[test]
    fn group_sits_mid_span_and_radius_caps() {
        assert_eq!(group_radius(2), 9.0);
        assert_eq!(group_radius(4), 11.0);
        assert_eq!(group_radius(1000), 13.0);
        assert_eq!(group_radius(1000), GROUP_MAX_RADIUS_PX);
        // The middle of the members' span (213), not their average (209.3).
        let xs = at_px([200.0, 202.0, 226.0]);
        assert_eq!(
            layout(&view(), &xs, 0..3, &[]),
            vec![Item::Group {
                members: 0..3,
                x: 213.0,
                radius: group_radius(3)
            }]
        );
    }

    #[test]
    fn pinned_never_grouped() {
        // Twenty saves 5 px apart, from 100 to 195 px, with "you are here" (10, at 150) inside.
        let xs = evenly(20, 5.0);
        let head = 10;
        let head_x = view().world_to_screen(xs[head]);
        let items = layout(&view(), &xs, 0..20, &[head]);
        assert!(items.contains(&single(head, head_x)), "{items:?}");

        let groups: Vec<(Range<usize>, f64, f64)> = items
            .iter()
            .filter_map(|item| match item {
                Item::Group { members, x, radius } => Some((members.clone(), *x, *radius)),
                Item::Single { .. } => None,
            })
            .collect();
        // Groups form on each side of it.
        assert!(groups.iter().any(|(m, ..)| m.end == head), "{items:?}");
        assert!(
            groups.iter().any(|(m, ..)| m.start == head + 1),
            "{items:?}"
        );
        for (members, x, radius) in &groups {
            assert!(!members.contains(&head));
            // Nudged clear of HEAD's ball, and kept on their own side of it.
            let distance = (x - head_x).abs();
            assert!(
                distance >= radius + BALL_RADIUS_PX,
                "{members:?} at {x} overlaps HEAD at {head_x}"
            );
            assert_eq!(*x < head_x, members.end <= head);
        }
        // Every save is still drawn once, as itself or in a group.
        let drawn: usize = items.iter().map(|item| members(item).len()).sum();
        assert_eq!(drawn, 20);

        // The selected save is pinned the same way.
        let items = layout(&view(), &xs, 0..20, &[head, 3]);
        assert!(items.contains(&single(3, view().world_to_screen(xs[3]))));
        assert!(items.contains(&single(head, head_x)));
    }

    /// Where grouping puts `item`, before separation: at its save, or mid-span of its members.
    fn natural_x(item: &Item, xs: &[Days], view: &View) -> f64 {
        let m = members(item);
        view.world_to_screen(xs[m.start])
            .midpoint(view.world_to_screen(xs[m.end - 1]))
    }

    /// Checks what separation promises: pinned balls sit at their own save, x never decreases,
    /// and every two balls that overlap (not only neighbours) include a pinned one.
    fn assert_separated(items: &[Item], xs: &[Days], view: &View, pinned: &[usize], context: &str) {
        let is_pinned =
            |item: &Item| matches!(*item, Item::Single { index, .. } if pinned.contains(&index));
        for item in items.iter().filter(|item| is_pinned(item)) {
            assert_eq!(
                item.x(),
                natural_x(item, xs, view),
                "{context}: pinned moved"
            );
        }
        for pair in items.windows(2) {
            assert!(
                pair[0].x() <= pair[1].x(),
                "{context}: out of order {pair:?}"
            );
        }
        for (i, a) in items.iter().enumerate() {
            // In order, so past the widest possible reach nothing else can overlap `a`.
            let reach = a.radius() + GROUP_MAX_RADIUS_PX + 2.0;
            for b in items[i + 1..].iter().take_while(|b| b.x() - a.x() < reach) {
                assert!(
                    apart(a, b) || is_pinned(a) || is_pinned(b),
                    "{context}: {a:?} overlaps {b:?}"
                );
            }
        }
    }

    #[test]
    fn neighbours_never_overlap() {
        // How many balls separation moved, so that the test is known to have something to check.
        let mut moved = 0;
        for seed in 1..=3 {
            let xs = random_xs(seed, 2_000);
            let head = xs.len() - 1;
            for day_px in [MIN_DAY_PX, 30.0, DEFAULT_DAY_PX, 2_000.0, MAX_DAY_PX] {
                let view = View {
                    day_px,
                    left: xs[0],
                    width: 1000.0,
                };
                let pinned_sets = [
                    vec![],
                    vec![head],
                    vec![700],
                    vec![head, 700],
                    // A selected save right next to "you are here", and two close together.
                    vec![head, head - 3],
                    vec![700, 702],
                ];
                for pinned in pinned_sets {
                    let items = layout(&view, &xs, 0..xs.len(), &pinned);
                    let context = format!("seed {seed}, {day_px} px a day, pinned {pinned:?}");
                    assert_separated(&items, &xs, &view, &pinned, &context);
                    // With one pinned ball or none, no overlap is left at all.
                    if pinned.len() < 2 {
                        for pair in items.windows(2) {
                            assert!(apart(&pair[0], &pair[1]), "{context}: {pair:?}");
                        }
                    }
                    let was_moved = |item: &&Item| item.x() != natural_x(item, &xs, &view);
                    moved += items.iter().filter(was_moved).count();
                }
            }
        }
        assert!(moved > 100, "only {moved} balls moved");
        // Fifty saves at the very same time make one ball.
        let xs = vec![1.0; 50];
        let items = layout(&view(), &xs, 0..50, &[]);
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn pinned_never_moves_in_separation() {
        // Sixty saves 3 px apart, from 100 to 277 px, with pinned saves 20 and 23 (at 160 and
        // 169): the pair of saves between them doesn't fit there, nor do the groups on either
        // side where grouping put them.
        let xs = evenly(60, 3.0);
        let pinned = [20, 23];
        let items = layout(&view(), &xs, 0..60, &pinned);
        assert_separated(&items, &xs, &view(), &pinned, "3 px apart");
        let x_of = |members: Range<usize>| {
            items
                .iter()
                .find(|item| self::members(item) == members)
                .map(Item::x)
                .unwrap_or_else(|| panic!("no ball for {members:?} in {items:?}"))
        };
        // The others moved away from them: the group just before 160 to the left of its middle
        // (143.5 px), the one just after 169 to the right of its middle (185.5 px).
        assert!(x_of(10..20) < 143.5);
        assert!(x_of(24..34) > 185.5);
        // The pair of saves between them stays between them, on top of them, and clear of the
        // groups on either side.
        let squeezed = x_of(21..23);
        assert!((160.0..=169.0).contains(&squeezed), "{items:?}");
    }

    #[test]
    fn squeezed_save_stays_between_its_pinned_neighbours() {
        // Ten saves 3 px apart (one group), then a peeked save at 130 px, four saves within the
        // next pixel, and "you are here" at 131 px.
        let xs = at_px([
            100.0, 103.0, 106.0, 109.0, 112.0, 115.0, 118.0, 121.0, 124.0, 127.0, 130.0, 130.2,
            130.4, 130.6, 130.8, 131.0,
        ]);
        let (peeked, head) = (10, 15);
        let pinned = [head, peeked];
        let items = layout(&view(), &xs, 0..xs.len(), &pinned);
        assert_separated(&items, &xs, &view(), &pinned, "peek next to head");
        let drawn: Vec<Range<usize>> = items.iter().map(members).collect();
        assert_eq!(drawn, [0..10, 10..11, 11..15, 15..16]);
        // The four saves stay between the peeked save and "you are here", never left of the
        // peeked one, and the group of ten keeps clear of them.
        let peeked_x = view().world_to_screen(xs[peeked]);
        let head_x = view().world_to_screen(xs[head]);
        assert!((peeked_x..=head_x).contains(&items[2].x()), "{items:?}");
        assert!(apart(&items[0], &items[2]), "{items:?}");
    }

    #[test]
    fn small_groups_are_as_easy_to_hit_as_singles() {
        let pair = Item::Group {
            members: 0..2,
            x: 100.0,
            radius: group_radius(2),
        };
        let big = Item::Group {
            members: 2..40,
            x: 200.0,
            radius: GROUP_MAX_RADIUS_PX,
        };
        let items = [pair, big];
        let hit = |x: f64| hit_test(&items, &[], 1_000.0, x, LINE_Y, LINE_Y);
        // A group of two (radius 9) reaches 12 px, like a single save.
        assert_eq!(hit(112.0), Some(Hit::Group(0..2)));
        assert_eq!(hit(88.0), Some(Hit::Group(0..2)));
        assert_eq!(hit(113.0), None);
        // A big group reaches its radius, 13 px.
        assert_eq!(hit(213.0), Some(Hit::Group(2..40)));
        assert_eq!(hit(214.0), None);
    }

    #[test]
    fn pinned_ball_wins_inside_its_painted_radius() {
        // A group squeezed under the pinned save 5, as between two pinned saves.
        let items = vec![
            Item::Group {
                members: 0..5,
                x: 100.0,
                radius: GROUP_MAX_RADIUS_PX,
            },
            single(5, 110.0),
            single(6, 200.0),
        ];
        let hit = |x: f64, pinned: &[usize]| hit_test(&items, pinned, 1_000.0, x, LINE_Y, LINE_Y);
        // Nearer the group, but inside the pinned ball as it is painted: the pinned ball.
        assert_eq!(hit(103.0, &[5]), Some(Hit::Save(5)));
        assert_eq!(hit(96.0, &[5]), Some(Hit::Save(5)));
        assert_eq!(hit(110.0 - PINNED_RADIUS_PX, &[5]), Some(Hit::Save(5)));
        // Past its radius, the nearest again.
        assert_eq!(hit(94.0, &[5]), Some(Hit::Group(0..5)));
        // Not pinned, the nearest wins.
        assert_eq!(hit(103.0, &[]), Some(Hit::Group(0..5)));
        // A pinned ball reaches as far as it is painted, further than a single save.
        assert_eq!(hit(200.0 + PINNED_RADIUS_PX, &[6]), Some(Hit::Save(6)));
        assert_eq!(hit(200.0 + PINNED_RADIUS_PX, &[]), None);
    }

    #[test]
    fn unpinned_balls_keep_clear_of_a_pinned_ball_as_painted() {
        // A group of four saves 3 px apart, then a pinned save 4 px after them, and a single
        // save 2 px after that.
        let xs = at_px([100.0, 103.0, 106.0, 109.0, 113.0, 115.0]);
        let pinned = [4];
        let items = layout(&view(), &xs, 0..6, &pinned);
        let drawn: Vec<Range<usize>> = items.iter().map(members).collect();
        assert_eq!(drawn, [0..4, 4..5, 5..6]);
        assert_eq!(items[1].x(), 113.0);
        let clearance = |item: &Item| (item.x() - 113.0).abs() - item.radius();
        // Both keep the pinned ball's painted radius and 2 px between them and it.
        assert!(
            clearance(&items[0]) >= PINNED_RADIUS_PX + 2.0 - 1e-9,
            "{items:?}"
        );
        assert!(
            clearance(&items[2]) >= PINNED_RADIUS_PX + 2.0 - 1e-9,
            "{items:?}"
        );
    }

    #[test]
    fn hit_prefers_nearest() {
        let items = vec![
            single(0, 100.0),
            single(1, 115.0),
            Item::Group {
                members: 2..5,
                x: 135.0,
                radius: 11.0,
            },
        ];
        let now_x = 200.0;
        let hit = |x: f64, y: f64| hit_test(&items, &[], now_x, x, y, LINE_Y);
        // Within reach of two balls, the nearer one wins.
        assert_eq!(hit(106.0, LINE_Y), Some(Hit::Save(0)));
        assert_eq!(hit(109.0, LINE_Y), Some(Hit::Save(1)));
        assert_eq!(hit(124.0, LINE_Y), Some(Hit::Save(1)));
        assert_eq!(hit(126.0, LINE_Y), Some(Hit::Group(2..5)));
        // A single reaches 12 px, and so does this group, whose radius is less.
        assert_eq!(hit(103.0, LINE_Y), Some(Hit::Save(0)));
        assert_eq!(hit(88.0, LINE_Y), Some(Hit::Save(0)));
        assert_eq!(hit(87.0, LINE_Y), None);
        assert_eq!(hit(147.0, LINE_Y), Some(Hit::Group(2..5)));
        assert_eq!(hit(148.0, LINE_Y), None);
        // "Agora" reaches 12 px.
        assert_eq!(hit(195.0, LINE_Y + 5.0), Some(Hit::Now));
        assert_eq!(hit(212.0, LINE_Y), Some(Hit::Now));
        assert_eq!(hit(213.0, LINE_Y), None);
        // Distance counts in y too: 9 px right and 9 down is 12.7 px away.
        assert_eq!(hit(100.0, LINE_Y + 12.0), Some(Hit::Save(0)));
        assert_eq!(hit(91.0, LINE_Y + 9.0), None);
        assert_eq!(hit(92.0, LINE_Y + 8.0), Some(Hit::Save(0)));
        assert_eq!(hit_test(&[], &[], now_x, 100.0, LINE_Y, LINE_Y), None);
    }

    #[test]
    fn visible_range_binary_search() {
        // One save a day for 10,000 days; the view shows days 5,000 to 5,100.
        let xs: Vec<Days> = (0..10_000).map(f64::from).collect();
        let view = View {
            day_px: 10.0,
            left: 5_000.0,
            width: 1000.0,
        };
        // With one screen width each side: days 4,900 to 5,200.
        assert_eq!(visible_range(&xs, &view), 4_900..5_201);
        assert_eq!(visible_range(&[], &view), 0..0);

        // The same saves as looking at each one, on uneven saves and past either end.
        let uneven: Vec<Days> = (0..3_000).map(|i| f64::from(i * i) / 900.0).collect();
        for left in [-500.0, -150.0, 0.0, 3.3, 77.7, 2_000.0, 9_950.0, 20_000.0] {
            let view = View {
                day_px: 10.0,
                left,
                width: 1000.0,
            };
            let near = view.screen_to_world(-view.width)..=view.screen_to_world(2.0 * view.width);
            let expected: Vec<usize> = (0..uneven.len())
                .filter(|&i| near.contains(&uneven[i]))
                .collect();
            let range = visible_range(&uneven, &view);
            assert_eq!(range.collect::<Vec<_>>(), expected, "left {left}");
        }
    }

    #[test]
    fn layout_is_fast() {
        // 10,000 saves a minute apart, all on screen at the deepest zoom out.
        let xs: Vec<Days> = (0..10_000).map(|i| f64::from(i) / 1440.0).collect();
        let view = View {
            day_px: MIN_DAY_PX,
            left: -1.0,
            width: 1000.0,
        };
        let head = xs.len() - 1;
        let items = timed_layout(&view, &xs, &[head]);
        let drawn: Vec<Range<usize>> = items.iter().map(members).collect();
        assert_eq!(drawn, [0..head, head..head + 1]);

        // Heavy separation at 32 px a day (a power of two, so that the px below are exact): a
        // group of 8 saves spanning 27 px, then 1,249 stacks of 8 saves made at the same time,
        // 28 px apart. Each stack is pushed right by the one before it, and two pinned saves
        // close together at the end hold the last ones back.
        let day_px = 32.0;
        let mut xs: Vec<Days> = (0..8).map(|i| f64::from(i) * 27.0 / 7.0 / day_px).collect();
        for stack in 1..1_250 {
            xs.extend(std::iter::repeat_n(f64::from(stack) * 28.0 / day_px, 8));
        }
        let view = View {
            day_px,
            left: 0.0,
            width: 1000.0,
        };
        let pinned = [xs.len() - 1, xs.len() - 4];
        let items = timed_layout(&view, &xs, &pinned);
        assert_separated(&items, &xs, &view, &pinned, "stacks");
        let was_moved = |item: &&Item| item.x() != natural_x(item, &xs, &view);
        let moved = items.iter().filter(was_moved).count();
        assert!(moved > 1_200, "{} balls, {moved} moved", items.len());
    }

    /// Lays out all of `xs` and checks it took under 5 ms: the best of three runs, so that a
    /// busy machine pausing the test once can't fail it.
    fn timed_layout(view: &View, xs: &[Days], pinned: &[usize]) -> Vec<Item> {
        let mut best = Duration::MAX;
        let mut items = Vec::new();
        for _ in 0..3 {
            let start = Instant::now();
            items = layout(view, xs, 0..xs.len(), pinned);
            best = best.min(start.elapsed());
        }
        assert!(best < Duration::from_millis(5), "took {best:?}");
        items
    }

    #[test]
    fn layout_range_gives_the_balls_of_the_whole_history() {
        // A dense run, 5 px apart, from 3,000 px left of the screen to 3,000 px right of it.
        let dense = at_px((0..1_200).map(|i| -3_000.0 + 5.0 * f64::from(i)));
        // The same with gaps of 150 px at -2,000 px and at 2,500 px, where the layout starts
        // afresh.
        let gapped: Vec<Days> = dense
            .iter()
            .enumerate()
            .map(|(i, &x)| {
                let shift = match i {
                    0..200 => 0.0,
                    200..1_100 => 145.0,
                    _ => 290.0,
                };
                x + shift / DAY_PX
            })
            .collect();
        let v = view();
        assert_eq!(visible_range(&dense, &v), 400..1_001);
        assert_eq!(layout_range(&v, &dense), 0..1_200);
        assert_eq!(layout_range(&v, &gapped), 200..1_100);
        assert_eq!(layout_range(&v, &[]), 0..0);
        let far = View { left: 1_000.0, ..v };
        assert!(layout_range(&far, &dense).is_empty());

        // Starting from the visible range alone puts other groups on screen...
        let visible = layout(&v, &dense, visible_range(&dense, &v), &[]);
        let whole = layout(&v, &dense, 0..dense.len(), &[]);
        assert_ne!(on_screen(&visible, &v), on_screen(&whole, &v));
        // ...but from the layout range, the balls on screen stay those of the whole history.
        let same_on_screen = |xs: &[Days], pinned: &[usize], view: &View| {
            let laid = layout(view, xs, layout_range(view, xs), pinned);
            let whole = layout(view, xs, 0..xs.len(), pinned);
            assert_eq!(on_screen(&laid, view), on_screen(&whole, view));
        };
        for xs in [&dense, &gapped] {
            for pinned in [
                vec![],
                vec![650],
                vec![300, 1_199],
                vec![201, 203],
                vec![1_098, 1_100],
            ] {
                let mut panned = v;
                for _ in 0..40 {
                    panned.pan_by(-7.0);
                    same_on_screen(xs, &pinned, &panned);
                }
            }
        }
        // Also on uneven saves, where separating the balls moves many of them.
        for seed in 1..=3 {
            let xs = random_xs(seed, 1_500);
            let head = xs.len() - 1;
            for day_px in [MIN_DAY_PX, 30.0, DEFAULT_DAY_PX, 2_000.0] {
                let pinned_sets = [
                    vec![],
                    vec![head],
                    vec![head, 600],
                    vec![head, head - 3],
                    vec![690, 692],
                ];
                for pinned in pinned_sets {
                    let mut panned = View {
                        day_px,
                        left: xs[700],
                        width: 1000.0,
                    };
                    for _ in 0..20 {
                        panned.pan_by(-37.0);
                        same_on_screen(&xs, &pinned, &panned);
                    }
                }
            }
        }
    }
}
