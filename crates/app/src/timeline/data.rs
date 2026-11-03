//! Turns what core reads of every timeline ([`TimelinesView`]) and where it draws each save
//! ([`LaneLayout`]) into the timeline's model data ([`TimelineData`]): the saves lane by lane,
//! each lane's name and color, and the selection carried over by save id.

use std::collections::HashSet;

use savestates_core::{GraphSave, LaneLayout, LaneOwner, SaveId, Timeline, TimelinesView};

use super::model::{LaneData, Selection, TimelineData};
use crate::theme::{self, lane_color};

/// The translated names of lanes that have none of their own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneNames {
    /// The main line without a description.
    pub main_line: String,
    /// The lane of a save the project is on outside any timeline (detached HEAD).
    pub no_line: String,
}

/// The timeline's data for `view` drawn as `layout` says: the saves lane by lane in display
/// order, each lane oldest first, with every index (forks, tags, "Você está aqui") moved to
/// that order. Saves in no lane are left out. The selection is `selected`'s save when it is
/// still drawn, else "Agora".
pub fn timeline_data(
    view: &TimelinesView,
    layout: &LaneLayout,
    names: &LaneNames,
    unsaved: usize,
    selected: Option<&SaveId>,
) -> TimelineData {
    // Where each of `view.saves` goes in the new order.
    let mut moved: Vec<Option<usize>> = vec![None; view.saves.len()];
    let mut saves = Vec::new();
    let mut ranges = Vec::with_capacity(layout.lanes.len());
    for lane in &layout.lanes {
        let start = saves.len();
        for &i in &lane.saves {
            if let Some(graph) = view.saves.get(i) {
                moved[i] = Some(saves.len());
                saves.push(graph.save.clone());
            }
        }
        ranges.push(start..saves.len());
    }
    let to_new = |i: usize| moved.get(i).copied().flatten();

    let lanes = layout
        .lanes
        .iter()
        .zip(ranges)
        .map(|(lane, range)| {
            let (label, branch, color, archived) = match &lane.owner {
                LaneOwner::Branch(branch) => {
                    let timeline = find(view, branch);
                    let main_line = timeline.is_some_and(|t| t.main_line);
                    (
                        label_of(branch, timeline, names),
                        Some(branch.clone()),
                        lane_color(branch, main_line),
                        timeline.is_some_and(|t| t.archived),
                    )
                }
                // Not a timeline: a neutral color, which no timeline has.
                LaneOwner::DetachedHead => (names.no_line.clone(), None, theme::MUTED_TEXT, false),
            };
            LaneData {
                label,
                branch,
                color,
                saves: range,
                fork: lane.fork.and_then(to_new),
                continues_left: lane.continues_past_loaded,
                archived,
            }
        })
        .collect();

    let tags = layout
        .tags
        .iter()
        .filter_map(|tag| {
            let save = to_new(tag.save)?;
            Some((save, label_of(&tag.branch, find(view, &tag.branch), names)))
        })
        .collect();

    let selection = selected
        .and_then(|id| saves.iter().position(|save| save.id == *id))
        .map_or(Selection::Now, Selection::Station);

    TimelineData {
        lanes,
        tags,
        here: layout.here.and_then(to_new),
        current_lane: layout.current_lane,
        pending_after: layout.pending_after,
        more_available: view.more_available,
        unsaved,
        selection,
        saves,
    }
}

/// Adds a page of `older` saves (newest first, as core reads them) after the loaded ones,
/// skipping those already loaded: pages read along several timelines overlap.
pub fn merge_older(view: &mut TimelinesView, older: Vec<GraphSave>, more_available: bool) {
    let mut loaded: HashSet<SaveId> = view.saves.iter().map(|s| s.save.id.clone()).collect();
    for save in older {
        if loaded.insert(save.save.id.clone()) {
            view.saves.push(save);
        }
    }
    view.more_available = more_available;
}

fn find<'a>(view: &'a TimelinesView, branch: &str) -> Option<&'a Timeline> {
    view.timelines.iter().find(|t| t.branch == branch)
}

/// A timeline's name: the one the user gave it, else the main line's own name for the main
/// line, else its git name.
fn label_of(branch: &str, timeline: Option<&Timeline>, names: &LaneNames) -> String {
    match timeline {
        Some(Timeline {
            display_name: Some(name),
            ..
        }) => name.clone(),
        Some(t) if t.main_line => names.main_line.clone(),
        _ => branch.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};
    use savestates_core::{
        GraphSave, Lane, LaneLayout, LaneOwner, SaveId, SaveState, Tag, Timeline, TimelinesView,
    };

    use crate::theme::{self, lane_color};
    use crate::timeline::model::Selection;

    fn names() -> LaneNames {
        LaneNames {
            main_line: "Linha principal".to_owned(),
            no_line: "Fora de uma linha".to_owned(),
        }
    }

    /// A save named `name`, made `hour` hours into 2 November 2026 (UTC-3).
    fn graph_save(name: &str, hour: u32, parent: Option<&str>) -> GraphSave {
        let time = FixedOffset::west_opt(3 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 11, 2, hour, 0, 0)
            .unwrap();
        GraphSave {
            save: SaveState {
                id: SaveId(name.to_owned()),
                name: name.to_owned(),
                time,
                committed: time,
                files_changed: 1,
            },
            parents: parent.map(|p| SaveId(p.to_owned())).into_iter().collect(),
        }
    }

    fn timeline(branch: &str, tip: &str, display_name: Option<&str>, main_line: bool) -> Timeline {
        Timeline {
            branch: branch.to_owned(),
            tip: SaveId(tip.to_owned()),
            display_name: display_name.map(str::to_owned),
            archived: false,
            main_line,
            current: false,
            elsewhere: None,
        }
    }

    /// `main` holds m1 m2 m3; `savestates/x` branches off m2 with x1 x2, and is current.
    /// `backup` points at m1 and has no save of its own: a tag.
    fn two_lanes() -> (TimelinesView, LaneLayout) {
        let mut x = timeline("savestates/x", "x2", None, false);
        x.current = true;
        let view = TimelinesView {
            timelines: vec![
                timeline("backup", "m1", None, false),
                timeline("main", "m3", None, true),
                x,
            ],
            head: Some(SaveId("x2".to_owned())),
            detached: false,
            // Newest first.
            saves: vec![
                graph_save("x2", 9, Some("x1")),
                graph_save("m3", 8, Some("m2")),
                graph_save("x1", 7, Some("m2")),
                graph_save("m2", 6, Some("m1")),
                graph_save("m1", 5, None),
            ],
            more_available: true,
        };
        let layout = LaneLayout {
            lanes: vec![
                Lane {
                    owner: LaneOwner::Branch("main".to_owned()),
                    saves: vec![4, 3, 1],
                    fork: None,
                    continues_past_loaded: false,
                },
                Lane {
                    owner: LaneOwner::Branch("savestates/x".to_owned()),
                    saves: vec![2, 0],
                    fork: Some(3),
                    continues_past_loaded: false,
                },
            ],
            tags: vec![Tag {
                branch: "backup".to_owned(),
                save: 4,
            }],
            here: Some(0),
            current_lane: Some(1),
            pending_after: None,
        };
        (view, layout)
    }

    fn save_names(data: &TimelineData) -> Vec<&str> {
        data.saves.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn saves_are_lane_by_lane_oldest_first() {
        let (view, layout) = two_lanes();
        let data = timeline_data(&view, &layout, &names(), 4, None);
        assert_eq!(save_names(&data), ["m1", "m2", "m3", "x1", "x2"]);
        assert_eq!(data.lanes.len(), 2);
        assert_eq!(data.lanes[0].saves, 0..3);
        assert_eq!(data.lanes[1].saves, 3..5);
        // The fork is the index of m2 in the new order.
        assert_eq!(data.lanes[1].fork, Some(1));
        assert_eq!(data.lanes[0].fork, None);
        // "Você está aqui", the tags and the lanes follow the new order too.
        assert_eq!(data.here, Some(4));
        assert_eq!(data.you_are_here(), Some(4));
        assert_eq!(data.tags, [(0, "backup".to_owned())]);
        assert_eq!(data.current_lane, Some(1));
        assert_eq!(data.pending_after, None);
        assert!(data.more_available);
        assert_eq!(data.unsaved, 4);
        assert_eq!(data.lanes[0].branch.as_deref(), Some("main"));
        assert_eq!(data.lanes[1].branch.as_deref(), Some("savestates/x"));
        // Colors come from the git names.
        assert_eq!(data.lanes[0].color, theme::LINE);
        assert_eq!(data.lanes[1].color, lane_color("savestates/x", false));
        assert!(!data.lanes[1].archived);
    }

    #[test]
    fn labels_use_display_main_and_git_names() {
        let (mut view, mut layout) = two_lanes();
        view.timelines[2].display_name = Some("Tarde de domingo".to_owned());
        view.timelines.push(timeline("develop", "d1", None, false));
        view.saves.insert(0, graph_save("d1", 10, Some("m3")));
        // Every index moved by one with d1 in front.
        for lane in &mut layout.lanes {
            for i in &mut lane.saves {
                *i += 1;
            }
            if let Some(fork) = &mut lane.fork {
                *fork += 1;
            }
        }
        layout.here = Some(1);
        layout.tags[0].save = 5;
        layout.lanes.push(Lane {
            owner: LaneOwner::Branch("develop".to_owned()),
            saves: vec![0],
            fork: Some(2),
            continues_past_loaded: false,
        });
        let data = timeline_data(&view, &layout, &names(), 0, None);
        let labels: Vec<&str> = data.lanes.iter().map(|l| l.label.as_str()).collect();
        // A description wins; the main line without one has its own name; others their git name.
        assert_eq!(labels, ["Linha principal", "Tarde de domingo", "develop"]);
        assert_eq!(data.lanes[2].fork, Some(2));

        // A description on the main line wins too.
        view.timelines[1].display_name = Some("Versão do cliente".to_owned());
        let data = timeline_data(&view, &layout, &names(), 0, None);
        assert_eq!(data.lanes[0].label, "Versão do cliente");

        // The detached HEAD's lane.
        layout.lanes.push(Lane {
            owner: LaneOwner::DetachedHead,
            saves: Vec::new(),
            fork: None,
            continues_past_loaded: false,
        });
        let data = timeline_data(&view, &layout, &names(), 0, None);
        assert_eq!(data.lanes[3].label, "Fora de uma linha");
        assert_eq!(data.lanes[3].branch, None);
        assert_eq!(data.lanes[3].color, theme::MUTED_TEXT);

        // An archived current timeline says so.
        view.timelines[2].archived = true;
        let data = timeline_data(&view, &layout, &names(), 0, None);
        assert!(data.lanes[1].archived);
        assert!(!data.lanes[0].archived);
    }

    #[test]
    fn the_selection_follows_its_save() {
        let (view, layout) = two_lanes();
        let x1 = SaveId("x1".to_owned());
        let data = timeline_data(&view, &layout, &names(), 0, Some(&x1));
        assert_eq!(data.selection, Selection::Station(3));
        let gone = SaveId("gone".to_owned());
        let data = timeline_data(&view, &layout, &names(), 0, Some(&gone));
        assert_eq!(data.selection, Selection::Now);
        let data = timeline_data(&view, &layout, &names(), 0, None);
        assert_eq!(data.selection, Selection::Now);
    }

    #[test]
    fn older_saves_merge_without_duplicates() {
        let (mut view, _) = two_lanes();
        let ids = |view: &TimelinesView| -> Vec<String> {
            view.saves.iter().map(|s| s.save.id.0.clone()).collect()
        };
        // An older page that starts again at m1 (pages of different branches overlap).
        let older = vec![
            graph_save("m1", 5, None),
            graph_save("o2", 4, Some("o1")),
            graph_save("o1", 3, None),
        ];
        merge_older(&mut view, older, false);
        assert_eq!(ids(&view), ["x2", "m3", "x1", "m2", "m1", "o2", "o1"]);
        assert!(!view.more_available);
        // Nothing new: only whether more exist changes.
        merge_older(&mut view, vec![graph_save("o1", 3, None)], true);
        assert_eq!(ids(&view).len(), 7);
        assert!(view.more_available);
        // Duplicates within the page itself are dropped too.
        merge_older(
            &mut view,
            vec![graph_save("o0", 2, None), graph_save("o0", 2, None)],
            false,
        );
        assert_eq!(ids(&view), ["x2", "m3", "x1", "m2", "m1", "o2", "o1", "o0"]);
    }
}
