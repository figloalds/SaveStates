//! Which save is drawn in which lane, and in what order the lanes sit (spec §6.2, §6.3, §6.5).
//!
//! Pure: the layout depends only on the git data in a [`TimelinesView`], so it is the same after
//! a restart.

use std::cmp::Reverse;
use std::collections::HashMap;

use crate::timelines::TimelinesView;

/// Whose line a lane draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaneOwner {
    /// A local branch, by its git name.
    Branch(String),
    /// The save the project folder is on, when it is on no timeline (detached HEAD).
    DetachedHead,
}

/// One horizontal lane: a line of saves drawn together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lane {
    pub owner: LaneOwner,
    /// Indexes into [`TimelinesView::saves`], oldest first.
    pub saves: Vec<usize>,
    /// The save this lane branches off (an index into [`TimelinesView::saves`]), in another lane.
    pub fork: Option<usize>,
    /// Whether the lane's oldest save has a save before it that is not loaded yet.
    pub continues_past_loaded: bool,
}

/// A branch with no save of its own, drawn as a small name on the save it points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub branch: String,
    /// An index into [`TimelinesView::saves`].
    pub save: usize,
}

/// Where every visible save is drawn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaneLayout {
    /// In display order (spec §6.3): the main line first.
    pub lanes: Vec<Lane>,
    pub tags: Vec<Tag>,
    /// HEAD's save, as an index into [`TimelinesView::saves`].
    pub here: Option<usize>,
    /// The lane of the current timeline; `None` when it is pending.
    pub current_lane: Option<usize>,
    /// The lane the pending lane (spec §6.5) sits right after; `None` when there is none.
    pub pending_after: Option<usize>,
}

/// A visible timeline tip, or the detached HEAD.
struct Tip<'a> {
    save: usize,
    /// Who wins a save several tips share: main line, current, app-made, other, detached HEAD.
    rank: u8,
    branch: Option<&'a str>,
    current: bool,
}

/// Assigns every save reachable from a visible timeline through first parents to exactly one
/// lane, and puts the lanes in display order.
pub fn assign_lanes(view: &TimelinesView) -> LaneLayout {
    let saves = &view.saves;
    let count = saves.len();
    if count == 0 {
        return LaneLayout::default();
    }
    let index: HashMap<&str, usize> = saves
        .iter()
        .enumerate()
        .map(|(i, s)| (s.save.id.0.as_str(), i))
        .collect();
    let find = |oid: &str| index.get(oid).copied();
    // The first parent within the loaded saves, and whether a first parent exists unloaded.
    let first: Vec<Option<usize>> = saves
        .iter()
        .map(|s| s.parents.first().and_then(|p| find(&p.0)))
        .collect();
    let cut: Vec<bool> = saves
        .iter()
        .map(|s| s.parents.first().is_some_and(|p| find(&p.0).is_none()))
        .collect();

    // Visible tips, grouped by save, the owner of each save first.
    let mut tips: Vec<Tip> = view
        .timelines
        .iter()
        .filter(|t| !t.archived || t.current || t.main_line)
        .filter_map(|t| {
            Some(Tip {
                save: find(&t.tip.0)?,
                rank: if t.main_line {
                    0
                } else if t.current {
                    1
                } else if t.app_made() {
                    2
                } else {
                    3
                },
                branch: Some(t.branch.as_str()),
                current: t.current,
            })
        })
        .collect();
    let head = view.head.as_ref().and_then(|h| find(&h.0));
    if view.detached
        && let Some(save) = head
    {
        tips.push(Tip {
            save,
            rank: 4,
            branch: None,
            current: false,
        });
    }
    tips.sort_by(|a, b| (a.save, a.rank, a.branch).cmp(&(b.save, b.rank, b.branch)));

    // The saves on some visible line, through first parents: the branches' lines first, so the
    // saves only a detached HEAD reaches can be told apart.
    let mut chain = vec![false; count];
    let mut branch_chain = Vec::new();
    for pass_branches in [true, false] {
        for tip in tips.iter().filter(|t| t.branch.is_some() == pass_branches) {
            let mut at = Some(tip.save);
            while let Some(i) = at {
                if chain[i] {
                    break;
                }
                chain[i] = true;
                at = first[i];
            }
        }
        if pass_branches {
            branch_chain = chain.clone();
        }
    }

    // The main line's first-parent line.
    let main_tip = view
        .timelines
        .iter()
        .find(|t| t.main_line)
        .and_then(|t| find(&t.tip.0));
    let mut on_main = vec![false; count];
    let mut at = main_tip;
    while let Some(i) = at {
        on_main[i] = true;
        at = first[i];
    }

    // heir[p]: the child that keeps p in its lane. The main line's own child when there is
    // one, else the oldest child (ties by the smaller oid).
    // branch_child[p]: whether p has a child on a branch's line. A save with only a detached
    // HEAD's save on top of it still ends its branch's lane.
    let mut heir: Vec<Option<usize>> = vec![None; count];
    let mut branch_child = vec![false; count];
    for child in (0..count).filter(|&c| chain[c]) {
        let Some(parent) = first[child] else {
            continue;
        };
        if branch_chain[child] {
            branch_child[parent] = true;
        }
        if on_main[parent] && heir[parent].is_some_and(|h| on_main[h]) {
            continue;
        }
        let wins = on_main[child]
            || heir[parent].is_none_or(|h| {
                let key = |i: usize| (saves[i].save.committed, saves[i].save.id.0.as_str());
                key(child) < key(h)
            });
        if wins {
            heir[parent] = Some(child);
        }
    }

    // Lane ends and their owners; the other tips at a lane end, or inside a line, are tags.
    let mut is_end = vec![false; count];
    let mut ends: Vec<(usize, LaneOwner)> = Vec::new();
    let mut tags: Vec<Tag> = Vec::new();
    let mut start = 0;
    while start < tips.len() {
        let save = tips[start].save;
        let group_end = start + tips[start..].iter().take_while(|t| t.save == save).count();
        let group = &tips[start..group_end];
        let mut rest = group;
        if Some(save) == main_tip || !branch_child[save] {
            is_end[save] = true;
            let owner = match group[0].branch {
                Some(name) => LaneOwner::Branch(name.to_string()),
                None => LaneOwner::DetachedHead,
            };
            ends.push((save, owner));
            rest = &group[1..];
        }
        for tip in rest {
            if let (Some(name), false) = (tip.branch, tip.current) {
                tags.push(Tag {
                    branch: name.to_string(),
                    save,
                });
            }
        }
        start = group_end;
    }

    // Walk each end back through first parents while this line is the parent's heir.
    let mut lanes: Vec<Lane> = Vec::with_capacity(ends.len());
    let mut lane_of: Vec<Option<usize>> = vec![None; count];
    for (end, owner) in ends {
        let mut own = vec![end];
        let mut fork = None;
        let mut continues_past_loaded = false;
        let mut at = end;
        loop {
            match first[at] {
                Some(parent) if !is_end[parent] && heir[parent] == Some(at) => {
                    own.push(parent);
                    at = parent;
                }
                Some(parent) => {
                    fork = Some(parent);
                    break;
                }
                None => {
                    continues_past_loaded = cut[at];
                    break;
                }
            }
        }
        own.reverse();
        for &i in &own {
            lane_of[i] = Some(lanes.len());
        }
        lanes.push(Lane {
            owner,
            saves: own,
            fork,
            continues_past_loaded,
        });
    }

    // Display order: main first, then depth-first, the lanes forking off a lane right under
    // it, newest first by their oldest own save; lanes without a fork after main's subtree.
    let newest_first = |l: &usize| {
        let oldest = &saves[lanes[*l].saves[0]].save;
        (Reverse(oldest.committed), oldest.id.0.as_str())
    };
    let main_lane = main_tip.and_then(|t| lane_of[t]);
    let mut under: Vec<Vec<usize>> = vec![Vec::new(); lanes.len()];
    let mut roots: Vec<usize> = Vec::new();
    for (l, lane) in lanes.iter().enumerate() {
        match lane.fork.and_then(|f| lane_of[f]) {
            Some(parent) => under[parent].push(l),
            None if Some(l) != main_lane => roots.push(l),
            None => {}
        }
    }
    for list in &mut under {
        list.sort_by_key(newest_first);
    }
    roots.sort_by_key(newest_first);
    let mut order: Vec<usize> = Vec::with_capacity(lanes.len());
    for root in main_lane.into_iter().chain(roots) {
        let mut stack = vec![root];
        while let Some(l) = stack.pop() {
            order.push(l);
            stack.extend(under[l].iter().rev());
        }
    }
    let mut position = vec![0; lanes.len()];
    for (p, &l) in order.iter().enumerate() {
        position[l] = p;
    }
    let mut slots: Vec<Option<Lane>> = lanes.into_iter().map(Some).collect();
    let lanes: Vec<Lane> = order.iter().map(|&l| slots[l].take().unwrap()).collect();
    let lane_at = |save: usize| lane_of[save].map(|l| position[l]);

    // Where "Você está aqui" is, and the current or pending lane.
    let mut layout = LaneLayout {
        here: head,
        ..LaneLayout::default()
    };
    let current = view.timelines.iter().find(|t| t.current);
    if view.detached {
        layout.current_lane = lanes
            .iter()
            .position(|l| l.owner == LaneOwner::DetachedHead)
            .or_else(|| head.and_then(lane_at));
    } else if let Some((timeline, tip)) = current.and_then(|t| Some((t, find(&t.tip.0)?))) {
        layout.here = Some(tip);
        let owned = lanes
            .iter()
            .position(|l| l.owner == LaneOwner::Branch(timeline.branch.clone()));
        match owned {
            Some(lane) => layout.current_lane = Some(lane),
            // The last save of a line another branch owns (the main line's tip): on that line.
            None if is_end[tip] => layout.current_lane = lane_at(tip),
            None => layout.pending_after = lane_at(tip),
        }
    }
    tags.sort_by(|a, b| (a.save, &a.branch).cmp(&(b.save, &b.branch)));
    layout.lanes = lanes;
    layout.tags = tags;
    layout
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone};

    use super::*;
    use crate::model::SaveId;
    use crate::timeline::SaveState;
    use crate::timelines::{GraphSave, Timeline, TimelinesView};

    fn zone() -> FixedOffset {
        FixedOffset::west_opt(3 * 3600).unwrap()
    }

    fn at(text: &str) -> DateTime<FixedOffset> {
        let naive = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap();
        zone().from_local_datetime(&naive).single().unwrap()
    }

    fn graph_save(oid: &str, parents: &[&str], committed: DateTime<FixedOffset>) -> GraphSave {
        GraphSave {
            save: SaveState {
                id: SaveId(oid.to_string()),
                name: oid.to_string(),
                time: committed,
                committed,
                files_changed: 1,
            },
            parents: parents.iter().map(|p| SaveId(p.to_string())).collect(),
        }
    }

    /// Saves are `(oid, parents, "YYYY-MM-DD HH:MM")` in any order; they are listed newest first,
    /// a later entry counting as newer on a tie. Branches are `(name, tip)`.
    fn view(
        saves: &[(&str, &[&str], &str)],
        branches: &[(&str, &str)],
        main: &str,
        current: Option<&str>,
    ) -> TimelinesView {
        let mut saves: Vec<GraphSave> = saves
            .iter()
            .rev()
            .map(|(oid, parents, time)| graph_save(oid, parents, at(time)))
            .collect();
        saves.sort_by_key(|s| std::cmp::Reverse(s.save.committed));
        let mut timelines: Vec<Timeline> = branches
            .iter()
            .map(|(name, tip)| Timeline {
                branch: name.to_string(),
                tip: SaveId(tip.to_string()),
                display_name: None,
                archived: false,
                main_line: *name == main,
                current: Some(*name) == current,
                elsewhere: None,
            })
            .collect();
        timelines.sort_by(|a, b| a.branch.cmp(&b.branch));
        let head = current.map(|name| {
            let (_, tip) = branches.iter().find(|(n, _)| *n == name).unwrap();
            SaveId(tip.to_string())
        });
        TimelinesView {
            timelines,
            head,
            detached: false,
            saves,
            more_available: false,
        }
    }

    fn set(view: &mut TimelinesView, branch: &str, change: impl FnOnce(&mut Timeline)) {
        change(
            view.timelines
                .iter_mut()
                .find(|t| t.branch == branch)
                .unwrap(),
        );
    }

    fn make_current(view: &mut TimelinesView, branch: &str) {
        for t in &mut view.timelines {
            t.current = t.branch == branch;
        }
        let tip = view
            .timelines
            .iter()
            .find(|t| t.branch == branch)
            .unwrap()
            .tip
            .clone();
        view.head = Some(tip);
    }

    fn oid(view: &TimelinesView, index: usize) -> &str {
        &view.saves[index].save.id.0
    }

    fn lane_oids<'a>(layout: &LaneLayout, view: &'a TimelinesView, lane: usize) -> Vec<&'a str> {
        layout.lanes[lane]
            .saves
            .iter()
            .map(|&i| oid(view, i))
            .collect()
    }

    fn fork_oid<'a>(layout: &LaneLayout, view: &'a TimelinesView, lane: usize) -> Option<&'a str> {
        layout.lanes[lane].fork.map(|i| oid(view, i))
    }

    fn owners(layout: &LaneLayout) -> Vec<String> {
        layout
            .lanes
            .iter()
            .map(|l| match &l.owner {
                LaneOwner::Branch(name) => name.clone(),
                LaneOwner::DetachedHead => "HEAD".to_string(),
            })
            .collect()
    }

    fn lane_of(layout: &LaneLayout, branch: &str) -> Option<usize> {
        layout
            .lanes
            .iter()
            .position(|l| l.owner == LaneOwner::Branch(branch.to_string()))
    }

    fn tags<'a>(layout: &'a LaneLayout, view: &'a TimelinesView) -> Vec<(&'a str, &'a str)> {
        layout
            .tags
            .iter()
            .map(|t| (t.branch.as_str(), oid(view, t.save)))
            .collect()
    }

    fn main_line_saves() -> Vec<(&'static str, &'static [&'static str], &'static str)> {
        vec![
            ("m1", &[], "2026-11-01 08:00"),
            ("m2", &["m1"], "2026-11-01 09:00"),
            ("m3", &["m2"], "2026-11-01 09:30"),
            ("m4", &["m3"], "2026-11-01 13:00"),
            ("m5", &["m4"], "2026-11-01 14:00"),
        ]
    }

    #[test]
    fn a_single_line_is_one_lane() {
        let v = view(
            &main_line_saves()[..3],
            &[("main", "m3")],
            "main",
            Some("main"),
        );
        let layout = assign_lanes(&v);
        assert_eq!(layout.lanes.len(), 1);
        assert_eq!(lane_oids(&layout, &v, 0), ["m1", "m2", "m3"]);
        assert_eq!(layout.lanes[0].fork, None);
        assert!(!layout.lanes[0].continues_past_loaded);
        assert_eq!(layout.here.map(|i| oid(&v, i)), Some("m3"));
        assert_eq!(layout.current_lane, Some(0));
        assert_eq!(layout.pending_after, None);
        assert!(layout.tags.is_empty());
    }

    #[test]
    fn a_timeline_from_the_middle_forks_off_it() {
        let mut saves = main_line_saves()[..4].to_vec();
        saves.push(("x1", &["m2"], "2026-11-01 10:00"));
        saves.push(("x2", &["x1"], "2026-11-01 11:00"));
        let v = view(
            &saves,
            &[("main", "m4"), ("savestates/x", "x2")],
            "main",
            Some("savestates/x"),
        );
        let layout = assign_lanes(&v);
        assert_eq!(owners(&layout), ["main", "savestates/x"]);
        assert_eq!(lane_oids(&layout, &v, 0), ["m1", "m2", "m3", "m4"]);
        assert_eq!(lane_oids(&layout, &v, 1), ["x1", "x2"]);
        assert_eq!(fork_oid(&layout, &v, 1), Some("m2"));
        assert_eq!(layout.current_lane, Some(1));
        assert_eq!(layout.here.map(|i| oid(&v, i)), Some("x2"));
    }

    #[test]
    fn children_sit_under_their_parent_newest_first() {
        let mut saves = main_line_saves();
        saves.push(("a1", &["m2"], "2026-11-01 10:00"));
        saves.push(("a2", &["a1"], "2026-11-01 10:30"));
        saves.push(("c1", &["a1"], "2026-11-01 11:00"));
        saves.push(("b1", &["m3"], "2026-11-01 12:00"));
        let v = view(
            &saves,
            &[("main", "m5"), ("a", "a2"), ("b", "b1"), ("c", "c1")],
            "main",
            Some("main"),
        );
        let layout = assign_lanes(&v);
        assert_eq!(owners(&layout), ["main", "b", "a", "c"]);
        assert_eq!(lane_oids(&layout, &v, 2), ["a1", "a2"]);
        assert_eq!(fork_oid(&layout, &v, 2), Some("m2"));
        assert_eq!(lane_oids(&layout, &v, 3), ["c1"]);
        assert_eq!(fork_oid(&layout, &v, 3), Some("a1"));
        assert_eq!(fork_oid(&layout, &v, 1), Some("m3"));
    }

    #[test]
    fn shared_saves_stay_with_the_line_that_continued_first() {
        let branches = [("main", "m3"), ("a", "a1"), ("c", "c1")];
        let mut saves = main_line_saves()[..3].to_vec();
        saves.push(("s1", &["m1"], "2026-11-01 08:30"));
        saves.push(("a1", &["s1"], "2026-11-01 09:00"));
        saves.push(("c1", &["s1"], "2026-11-01 10:00"));
        let v = view(&saves, &branches, "main", Some("main"));
        let layout = assign_lanes(&v);
        let a = lane_of(&layout, "a").unwrap();
        let c = lane_of(&layout, "c").unwrap();
        assert_eq!(lane_oids(&layout, &v, a), ["s1", "a1"]);
        assert_eq!(fork_oid(&layout, &v, a), Some("m1"));
        assert_eq!(lane_oids(&layout, &v, c), ["c1"]);
        assert_eq!(fork_oid(&layout, &v, c), Some("s1"));

        let mut saves = main_line_saves()[..3].to_vec();
        saves.push(("s1", &["m1"], "2026-11-01 08:30"));
        saves.push(("c1", &["s1"], "2026-11-01 09:00"));
        saves.push(("a1", &["s1"], "2026-11-01 10:00"));
        let v = view(&saves, &branches, "main", Some("main"));
        let layout = assign_lanes(&v);
        let a = lane_of(&layout, "a").unwrap();
        let c = lane_of(&layout, "c").unwrap();
        assert_eq!(lane_oids(&layout, &v, c), ["s1", "c1"]);
        assert_eq!(lane_oids(&layout, &v, a), ["a1"]);
        assert_eq!(fork_oid(&layout, &v, a), Some("s1"));
    }

    #[test]
    fn a_branch_inside_another_line_is_a_tag() {
        let mut saves = main_line_saves()[..3].to_vec();
        saves.push(("d1", &["m1"], "2026-11-01 08:30"));
        saves.push(("f1", &["d1"], "2026-11-01 10:00"));
        let v = view(
            &saves,
            &[
                ("backup", "m2"),
                ("develop", "d1"),
                ("feature", "f1"),
                ("main", "m3"),
            ],
            "main",
            Some("main"),
        );
        let layout = assign_lanes(&v);
        assert_eq!(owners(&layout), ["main", "feature"]);
        assert_eq!(lane_oids(&layout, &v, 1), ["d1", "f1"]);
        assert_eq!(fork_oid(&layout, &v, 1), Some("m1"));
        let mut found = tags(&layout, &v);
        found.sort();
        assert_eq!(found, [("backup", "m2"), ("develop", "d1")]);
    }

    #[test]
    fn two_branches_at_one_tip_share_a_lane() {
        let mut saves = main_line_saves()[..3].to_vec();
        saves.push(("x1", &["m2"], "2026-11-01 10:00"));
        let v = view(
            &saves,
            &[("main", "m3"), ("savestates/x", "x1"), ("zeta", "x1")],
            "main",
            Some("main"),
        );
        let layout = assign_lanes(&v);
        assert_eq!(owners(&layout), ["main", "savestates/x"]);
        assert_eq!(lane_oids(&layout, &v, 1), ["x1"]);
        assert_eq!(tags(&layout, &v), [("zeta", "x1")]);
    }

    #[test]
    fn the_current_branch_sharing_a_non_main_tip_owns_the_lane() {
        let mut saves = main_line_saves()[..3].to_vec();
        saves.push(("x1", &["m2"], "2026-11-01 10:00"));
        let v = view(
            &saves,
            &[("main", "m3"), ("savestates/x", "x1"), ("zeta", "x1")],
            "main",
            Some("zeta"),
        );
        let layout = assign_lanes(&v);
        assert_eq!(owners(&layout), ["main", "zeta"]);
        assert_eq!(tags(&layout, &v), [("savestates/x", "x1")]);
        assert_eq!(layout.current_lane, Some(1));
        assert_eq!(layout.pending_after, None);
        assert_eq!(layout.here.map(|i| oid(&v, i)), Some("x1"));
    }

    #[test]
    fn the_current_branch_at_the_main_tip_is_on_the_main_lane() {
        let v = view(
            &main_line_saves()[..3],
            &[("main", "m3"), ("x", "m3")],
            "main",
            Some("x"),
        );
        let layout = assign_lanes(&v);
        assert_eq!(owners(&layout), ["main"]);
        assert_eq!(layout.current_lane, Some(0));
        assert_eq!(layout.pending_after, None);
        assert_eq!(layout.here.map(|i| oid(&v, i)), Some("m3"));
        assert!(layout.tags.is_empty());
    }

    #[test]
    fn a_detached_save_on_a_branch_tip_forks_off_it() {
        let mut saves = main_line_saves()[..3].to_vec();
        saves.push(("b1", &["m2"], "2026-11-01 10:00"));
        saves.push(("d1", &["b1"], "2026-11-01 11:00"));
        let mut v = view(&saves, &[("b", "b1"), ("main", "m3")], "main", None);
        v.detached = true;
        v.head = Some(SaveId("d1".into()));
        let layout = assign_lanes(&v);
        assert_eq!(owners(&layout), ["main", "b", "HEAD"]);
        assert_eq!(lane_oids(&layout, &v, 1), ["b1"]);
        assert_eq!(fork_oid(&layout, &v, 1), Some("m2"));
        assert_eq!(lane_oids(&layout, &v, 2), ["d1"]);
        assert_eq!(fork_oid(&layout, &v, 2), Some("b1"));
        assert!(layout.tags.is_empty());
        assert_eq!(layout.current_lane, Some(2));
        assert_eq!(layout.here.map(|i| oid(&v, i)), Some("d1"));
    }

    fn merge_saves() -> Vec<(&'static str, &'static [&'static str], &'static str)> {
        vec![
            ("m1", &[], "2026-11-01 08:00"),
            ("f1", &["m1"], "2026-11-01 09:00"),
            ("f2", &["f1"], "2026-11-01 10:00"),
            ("m2", &["m1"], "2026-11-01 11:00"),
            ("m3", &["m2", "f2"], "2026-11-01 12:00"),
        ]
    }

    #[test]
    fn a_merged_timeline_keeps_its_lane() {
        let v = view(
            &merge_saves(),
            &[("feature", "f2"), ("main", "m3")],
            "main",
            Some("main"),
        );
        let layout = assign_lanes(&v);
        assert_eq!(owners(&layout), ["main", "feature"]);
        assert_eq!(lane_oids(&layout, &v, 0), ["m1", "m2", "m3"]);
        assert_eq!(lane_oids(&layout, &v, 1), ["f1", "f2"]);
        assert_eq!(fork_oid(&layout, &v, 1), Some("m1"));
    }

    #[test]
    fn merged_saves_without_a_branch_are_not_drawn() {
        let v = view(&merge_saves(), &[("main", "m3")], "main", Some("main"));
        let layout = assign_lanes(&v);
        assert_eq!(owners(&layout), ["main"]);
        assert_eq!(lane_oids(&layout, &v, 0), ["m1", "m2", "m3"]);
        assert!(layout.tags.is_empty());
    }

    #[test]
    fn the_main_line_stays_on_top_even_when_branched_at_its_tip() {
        let mut saves = main_line_saves()[..3].to_vec();
        saves.push(("f1", &["m3"], "2026-11-01 10:00"));
        let v = view(
            &saves,
            &[("feature", "f1"), ("main", "m3")],
            "main",
            Some("feature"),
        );
        let layout = assign_lanes(&v);
        assert_eq!(owners(&layout), ["main", "feature"]);
        assert_eq!(lane_oids(&layout, &v, 0), ["m1", "m2", "m3"]);
        assert_eq!(lane_oids(&layout, &v, 1), ["f1"]);
        assert_eq!(fork_oid(&layout, &v, 1), Some("m3"));
        assert_eq!(layout.current_lane, Some(1));
    }

    #[test]
    fn a_just_loaded_timeline_is_pending() {
        let v = view(
            &main_line_saves()[..3],
            &[("main", "m3"), ("savestates/x", "m2")],
            "main",
            Some("savestates/x"),
        );
        let layout = assign_lanes(&v);
        assert_eq!(owners(&layout), ["main"]);
        assert_eq!(layout.pending_after, Some(0));
        assert_eq!(layout.here.map(|i| oid(&v, i)), Some("m2"));
        assert_eq!(layout.current_lane, None);
        assert!(layout.tags.is_empty());
    }

    #[test]
    fn archived_timelines_are_hidden_unless_current() {
        let mut saves = main_line_saves()[..3].to_vec();
        saves.push(("a1", &["m1"], "2026-11-01 08:30"));
        saves.push(("a2", &["a1"], "2026-11-01 09:10"));
        saves.push(("c1", &["a1"], "2026-11-01 10:00"));
        let mut v = view(
            &saves,
            &[("a", "a2"), ("c", "c1"), ("main", "m3")],
            "main",
            Some("main"),
        );
        set(&mut v, "a", |t| t.archived = true);
        let layout = assign_lanes(&v);
        assert_eq!(owners(&layout), ["main", "c"]);
        assert_eq!(lane_oids(&layout, &v, 1), ["a1", "c1"]);
        assert_eq!(fork_oid(&layout, &v, 1), Some("m1"));
        assert!(layout.tags.is_empty());

        make_current(&mut v, "a");
        let layout = assign_lanes(&v);
        let a = lane_of(&layout, "a").expect("the current timeline has a lane");
        assert_eq!(lane_oids(&layout, &v, a), ["a1", "a2"]);
        assert_eq!(layout.current_lane, Some(a));
        assert!(lane_of(&layout, "c").is_some());
    }

    #[test]
    fn a_detached_save_gets_its_own_lane() {
        let mut saves = main_line_saves()[..3].to_vec();
        saves.push(("d1", &["m2"], "2026-11-01 10:00"));
        let mut v = view(&saves, &[("main", "m3")], "main", None);
        v.detached = true;
        v.head = Some(SaveId("d1".into()));
        let layout = assign_lanes(&v);
        assert_eq!(owners(&layout), ["main", "HEAD"]);
        assert_eq!(lane_oids(&layout, &v, 1), ["d1"]);
        assert_eq!(fork_oid(&layout, &v, 1), Some("m2"));
        assert_eq!(layout.here.map(|i| oid(&v, i)), Some("d1"));
        assert_eq!(layout.current_lane, Some(1));
        assert_eq!(layout.pending_after, None);

        // Detached on a save inside a line: no lane of its own, it is on that line.
        v.head = Some(SaveId("m2".into()));
        let layout = assign_lanes(&v);
        assert_eq!(owners(&layout), ["main"]);
        assert_eq!(layout.here.map(|i| oid(&v, i)), Some("m2"));
        assert_eq!(layout.current_lane, Some(0));
    }

    #[test]
    fn a_line_leaving_the_loaded_saves_continues() {
        let mut saves = main_line_saves()[..3].to_vec();
        saves.push(("x1", &["gone"], "2026-11-01 07:00"));
        saves.push(("x2", &["x1"], "2026-11-01 10:00"));
        saves.push(("y1", &["m2"], "2026-11-01 11:00"));
        let v = view(
            &saves,
            &[("main", "m3"), ("x", "x2"), ("y", "y1")],
            "main",
            Some("main"),
        );
        let layout = assign_lanes(&v);
        assert_eq!(owners(&layout), ["main", "y", "x"]);
        assert_eq!(lane_oids(&layout, &v, 2), ["x1", "x2"]);
        assert_eq!(layout.lanes[2].fork, None);
        assert!(layout.lanes[2].continues_past_loaded);
        assert!(!layout.lanes[0].continues_past_loaded);
        assert!(!layout.lanes[1].continues_past_loaded);
    }

    #[test]
    fn nothing_saved_yet_gives_no_lanes() {
        let v = TimelinesView::default();
        assert_eq!(assign_lanes(&v), LaneLayout::default());
    }

    #[test]
    fn lanes_for_ten_thousand_saves_are_quick() {
        let base = at("2026-01-01 00:00");
        let mut saves = Vec::new();
        let mut timelines = Vec::new();
        for i in 0..10_000 {
            let parents: Vec<String> = if i == 0 {
                vec![]
            } else {
                vec![format!("m{}", i - 1)]
            };
            let parents: Vec<&str> = parents.iter().map(String::as_str).collect();
            let time = base + chrono::Duration::minutes(i);
            saves.push(graph_save(&format!("m{i}"), &parents, time));
        }
        timelines.push(Timeline {
            branch: "main".into(),
            tip: SaveId("m9999".into()),
            display_name: None,
            archived: false,
            main_line: true,
            current: true,
            elsewhere: None,
        });
        for k in 0..49i64 {
            let fork = (k + 1) * 200;
            for j in 0..20i64 {
                let parent = if j == 0 {
                    format!("m{fork}")
                } else {
                    format!("b{k}-{}", j - 1)
                };
                let time =
                    base + chrono::Duration::minutes(fork) + chrono::Duration::seconds(j + 1);
                saves.push(graph_save(&format!("b{k}-{j}"), &[&parent], time));
            }
            timelines.push(Timeline {
                branch: format!("b{k}"),
                tip: SaveId(format!("b{k}-19")),
                display_name: None,
                archived: false,
                main_line: false,
                current: false,
                elsewhere: None,
            });
        }
        saves.sort_by_key(|s| std::cmp::Reverse(s.save.committed));
        timelines.sort_by(|a, b| a.branch.cmp(&b.branch));
        let v = TimelinesView {
            timelines,
            head: Some(SaveId("m9999".into())),
            detached: false,
            saves,
            more_available: false,
        };

        let started = Instant::now();
        let layout = assign_lanes(&v);
        let took = started.elapsed();
        assert!(took < Duration::from_millis(50), "took {took:?}");

        assert_eq!(layout.lanes.len(), 50);
        let mut seen = vec![0u32; v.saves.len()];
        for lane in &layout.lanes {
            for &i in &lane.saves {
                seen[i] += 1;
            }
        }
        assert!(seen.iter().all(|&n| n == 1));
    }
}
