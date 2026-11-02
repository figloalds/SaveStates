//! Pure layout of the timeline's balls: which saves merge into a group ball at the current
//! zoom, where each ball sits on screen, and what the pointer is on. Nothing here draws.
//!
//! Saves come as world xs (see [`super::axis`]), oldest first and never decreasing. Walking
//! them in that order, a save joins the open group while it is closer than [`GROUP_JOIN_PX`]
//! to the group's *first* member, so a long chain of saves becomes several neighbouring groups
//! instead of one huge one. Pinned saves ("Você está aqui" and the selected save) always keep
//! their own ball: they split the walk. Last, neighbouring balls are moved apart so that none
//! overlap; pinned balls never move, so the others move away from them.

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

/// The space kept between neighbouring balls.
const BALL_GAP_PX: f64 = 2.0;
/// A gap between two saves wide enough that the balls before it can't move those after it, nor
/// the reverse (see [`layout_range`]). Separating neighbours (see [`separate`]) moves a ball
/// less than 22 px right and 36 px left of where grouping put it, and two neighbours need at
/// most 28 px between their centers: 22 + 36 + 28 = 86 px, and this is 112.
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
    fn x(&self) -> f64 {
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
/// point where the layout starts afresh whatever lies beyond it. That is the first or last
/// save, a pinned save, or a gap of [`RESYNC_GAP_PX`] between two saves. Laid out from there,
/// the balls on screen are those of the whole history, so they don't change as the edges of
/// the range move while panning.
pub fn layout_range(view: &View, xs: &[Days], pinned: &[usize]) -> Range<usize> {
    let visible = visible_range(xs, view);
    if visible.is_empty() {
        return visible;
    }
    let gap = |i: usize| view.world_to_screen(xs[i]) - view.world_to_screen(xs[i - 1]);
    let mut start = visible.start;
    while start > 0 && !pinned.contains(&start) && gap(start) < RESYNC_GAP_PX {
        start -= 1;
    }
    let mut end = visible.end;
    while end < xs.len() && !pinned.contains(&(end - 1)) && gap(end) < RESYNC_GAP_PX {
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
/// 5. Neighbouring balls are moved apart until no two overlap, with 2 px between them. Pinned
///    balls never move, so a group closer to one than its radius + [`BALL_RADIUS_PX`] + 2 px
///    moves away from it, to that distance (see [`separate`]).
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

/// Moves balls (in index order) apart so that each sits at least the sum of both radii +
/// [`BALL_GAP_PX`] after the one before it. Pinned balls never move.
///
/// A forward sweep pushes each ball that overlaps the one before it to the right. A backward
/// sweep then pushes each ball that overlaps the one after it to the left; after the forward
/// sweep that only happens before a pinned ball, which holds the balls before it back. Balls
/// that aren't pinned may drift from their time this way.
///
/// When two pinned balls are too close for the balls between them (or to each other), not every
/// overlap can go: the balls between them then overlap the pinned ball on their left, and the
/// painter draws pinned balls on top.
fn separate(items: &mut [Item], pinned: &[usize]) {
    let is_pinned =
        |item: &Item| matches!(*item, Item::Single { index, .. } if pinned.contains(&index));
    let spacing = |a: &Item, b: &Item| a.radius() + b.radius() + BALL_GAP_PX;
    for k in 1..items.len() {
        let least = items[k - 1].x() + spacing(&items[k - 1], &items[k]);
        if !is_pinned(&items[k]) && items[k].x() < least {
            *items[k].x_mut() = least;
        }
    }
    for k in (1..items.len()).rev() {
        let most = items[k].x() - spacing(&items[k - 1], &items[k]);
        if !is_pinned(&items[k - 1]) && items[k - 1].x() > most {
            *items[k - 1].x_mut() = most;
        }
    }
}

/// What the pointer at (`x`, `y`) is on: the nearest ball within its reach, if any. A single
/// save and "Agora" (at screen x `now_x`) reach [`HIT_RADIUS_PX`]; a group reaches its radius,
/// but never less than a single save. Every ball sits at `line_y`.
pub fn hit_test(items: &[Item], now_x: f64, x: f64, y: f64, line_y: f64) -> Option<Hit> {
    let dy = y - line_y;
    let mut best: Option<(Hit, f64)> = None;
    let mut consider = |hit: Hit, ball_x: f64, reach: f64| {
        let distance = (ball_x - x).hypot(dy);
        if distance <= reach && best.as_ref().is_none_or(|(_, nearest)| distance < *nearest) {
            best = Some((hit, distance));
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
                // Nothing pinned, "you are here" alone, or a selected save alone: every pair of
                // neighbours is apart.
                for pinned in [vec![], vec![head], vec![700]] {
                    let items = layout(&view, &xs, 0..xs.len(), &pinned);
                    let was_moved = |item: &&Item| item.x() != natural_x(item, &xs, &view);
                    moved += items.iter().filter(was_moved).count();
                    for pair in items.windows(2) {
                        assert!(
                            apart(&pair[0], &pair[1]),
                            "seed {seed}, {day_px} px a day, {pinned:?}: {pair:?}"
                        );
                    }
                }
                // With two pinned balls there may be no room between them: the only overlap
                // left is then with the pinned ball on the left of the crowd.
                let pinned = [head, 700];
                let items = layout(&view, &xs, 0..xs.len(), &pinned);
                for pair in items.windows(2) {
                    let after_pinned =
                        matches!(pair[0], Item::Single { index, .. } if pinned.contains(&index));
                    assert!(
                        apart(&pair[0], &pair[1]) || after_pinned,
                        "seed {seed}, {day_px} px a day: {pair:?}"
                    );
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
        // 169) between them: the pair of saves between them doesn't fit, nor do the groups on
        // either side where grouping put them.
        let xs = evenly(60, 3.0);
        let pinned = [20, 23];
        let items = layout(&view(), &xs, 0..60, &pinned);
        for p in pinned {
            assert!(
                items.contains(&single(p, view().world_to_screen(xs[p]))),
                "{items:?}"
            );
        }
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
        // Every overlap left is with the pinned ball on its left: here the pair of saves
        // between them, held back by the pinned ball on their right.
        for pair in items.windows(2) {
            let after_pinned =
                matches!(pair[0], Item::Single { index, .. } if pinned.contains(&index));
            assert!(apart(&pair[0], &pair[1]) || after_pinned, "{pair:?}");
        }
        assert!(!apart(&single(20, 160.0), &items[3]), "{items:?}");
        assert_eq!(members(&items[3]), 21..23);
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
        let hit = |x: f64| hit_test(&items, 1_000.0, x, LINE_Y, LINE_Y);
        // A group of two (radius 9) reaches 12 px, like a single save.
        assert_eq!(hit(112.0), Some(Hit::Group(0..2)));
        assert_eq!(hit(88.0), Some(Hit::Group(0..2)));
        assert_eq!(hit(113.0), None);
        // A big group reaches its radius, 13 px.
        assert_eq!(hit(213.0), Some(Hit::Group(2..40)));
        assert_eq!(hit(214.0), None);
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
        let hit = |x: f64, y: f64| hit_test(&items, now_x, x, y, LINE_Y);
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
        assert_eq!(hit_test(&[], now_x, 100.0, LINE_Y, LINE_Y), None);
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
        let mut best = Duration::MAX;
        let mut items = Vec::new();
        // The best of three runs, so that a busy machine pausing the test once can't fail it.
        for _ in 0..3 {
            let start = Instant::now();
            items = layout(&view, &xs, 0..xs.len(), &[head]);
            best = best.min(start.elapsed());
        }
        assert!(best < Duration::from_millis(5), "took {best:?}");
        let drawn: Vec<Range<usize>> = items.iter().map(members).collect();
        assert_eq!(drawn, [0..head, head..head + 1]);
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
        assert_eq!(layout_range(&v, &dense, &[]), 0..1_200);
        assert_eq!(layout_range(&v, &gapped, &[]), 200..1_100);
        // A pinned save starts afresh too.
        assert_eq!(layout_range(&v, &gapped, &[300]), 300..1_100);
        assert_eq!(layout_range(&v, &gapped, &[1_000]), 200..1_001);
        assert_eq!(layout_range(&v, &[], &[]), 0..0);
        let far = View { left: 1_000.0, ..v };
        assert!(layout_range(&far, &dense, &[]).is_empty());

        // Starting from the visible range alone puts other groups on screen...
        let visible = layout(&v, &dense, visible_range(&dense, &v), &[]);
        let whole = layout(&v, &dense, 0..dense.len(), &[]);
        assert_ne!(on_screen(&visible, &v), on_screen(&whole, &v));
        // ...but from the layout range, the balls on screen stay those of the whole history.
        let same_on_screen = |xs: &[Days], pinned: &[usize], view: &View| {
            let laid = layout(view, xs, layout_range(view, xs, pinned), pinned);
            let whole = layout(view, xs, 0..xs.len(), pinned);
            assert_eq!(on_screen(&laid, view), on_screen(&whole, view));
        };
        for xs in [&dense, &gapped] {
            for pinned in [vec![], vec![650], vec![300, 1_199]] {
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
                for pinned in [vec![], vec![head], vec![head, 600]] {
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
