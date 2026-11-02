//! The open project tabs: which folders are open and which one is active ([`TabsState`]), and
//! what one tab shows ([`ProjectTab`]).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use savestates_core::{
    CoreError, HISTORY_PAGE, Project, ProjectSnapshot, RepoCondition, SaveId, SaveState,
    prune_recovery, snapshot, timeline_page,
};
use savestates_i18n::Lang;
use savestates_ui_raven::{Widget, task, tr, tr_fmt, tr_plural, widget_from_xml};

use crate::app::with_tab;
use crate::banner::banner_keys;
use crate::start_screen::folder_name;
use crate::timeline::model::{Selection, TimelineAction, TimelineData};
use crate::timeline::widget::{self as timeline_widget, TimelineWidget};
use crate::ui_files;
use crate::watcher::{self, WatchHandle};
use crate::worker::{Poster, ProjectWorker};

/// What [`TabsState::open`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabOp {
    /// A new tab at this index.
    Added(usize),
    /// The folder was already open in the tab at this index.
    Activated(usize),
}

/// The folders open as tabs, in tab order, and the active one.
#[derive(Debug, Default)]
pub struct TabsState {
    roots: Vec<PathBuf>,
    active: Option<usize>,
}

impl TabsState {
    /// Opens `root` in a new tab, or activates the tab that already shows the same folder
    /// (however it is spelled: letter case, a trailing separator, `.` or `..`).
    pub fn open(&mut self, root: PathBuf) -> TabOp {
        let key = folder_key(&root);
        if let Some(i) = self.roots.iter().position(|r| folder_key(r) == key) {
            self.active = Some(i);
            return TabOp::Activated(i);
        }
        self.roots.push(root);
        let i = self.roots.len() - 1;
        self.active = Some(i);
        TabOp::Added(i)
    }

    /// Closes the tab at `idx`. When it was the active one, the tab that takes its place (or the
    /// one before it, for the last tab) becomes active.
    pub fn close(&mut self, idx: usize) {
        if idx >= self.roots.len() {
            return;
        }
        self.roots.remove(idx);
        self.active = match self.active {
            _ if self.roots.is_empty() => None,
            Some(a) if a > idx => Some(a - 1),
            Some(a) if a == idx => Some(idx.min(self.roots.len() - 1)),
            other => other,
        };
    }

    /// Makes the tab at `idx` the active one.
    pub fn activate(&mut self, idx: usize) {
        if idx < self.roots.len() {
            self.active = Some(idx);
        }
    }

    pub fn active(&self) -> Option<usize> {
        self.active
    }

    /// The folders, in tab order, as they were given to [`open`](Self::open).
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }
}

/// What two spellings of the same folder have in common: the resolved path when the folder
/// exists, then without trailing separators and, on Windows, without letter case.
fn folder_key(path: &Path) -> String {
    let resolved = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = resolved.to_string_lossy();
    let trimmed = text.trim_end_matches(['\\', '/']);
    // A drive root keeps its separator: `C:\` and `C:` are not the same folder.
    let key = if trimmed.ends_with(':') {
        text.as_ref()
    } else {
        trimmed
    };
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key.to_owned()
    }
}

/// Whether `a` and `b` name the same folder (see [`TabsState::open`]).
pub fn same_folder(a: &Path, b: &Path) -> bool {
    folder_key(a) == folder_key(b)
}

/// Tells the tabs apart in results that come back from other threads.
pub type TabId = u64;

/// A button of a tab that the app reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabButton {
    JumpToNow,
    CheckAgain,
    /// "Remove from list", in a tab whose folder is gone.
    Remove,
}

/// The widgets of a tab that change while it is open (`project_tab.xml`).
struct TabWidgets {
    count: Widget,
    jump: Widget,
    banner: Widget,
    banner_text: Widget,
    check_again: Widget,
    details: Widget,
    details_text: Widget,
    changes_count: Widget,
    quick_save: Widget,
    throw_away: Widget,
    remove: Widget,
}

/// What a tab whose folder is a project has.
struct Live {
    worker: ProjectWorker,
    timeline: TimelineWidget,
    /// Stops watching the folder when the tab closes. `None` when it could not be watched: the
    /// tab then refreshes only when the window comes back to the front or after an operation.
    _watch: Option<WatchHandle>,
}

struct TabState {
    /// Which save's history the timeline shows (`Some(None)`: a project without saves).
    /// `None` until the first page is loaded.
    history_head: Option<Option<SaveId>>,
    /// Saves older than the loaded ones exist.
    more_available: bool,
    /// Counts the times the timeline was given new data. A page of older saves asked for
    /// before one of them belongs to other data and is dropped.
    data_generation: u64,
    /// The selected station, as the timeline last reported it.
    selection: Selection,
}

/// One open tab: a project, or a folder that is gone (the "missing" state).
pub struct ProjectTab {
    pub id: TabId,
    pub root: PathBuf,
    /// The tab's page in the tab view.
    pub page: Widget,
    ui: TabWidgets,
    live: Option<Live>,
    state: RefCell<TabState>,
}

impl ProjectTab {
    /// A tab for `project`: its page, timeline, worker thread and folder watcher. Nothing is
    /// loaded until [`ProjectTab::refresh`]. `zoom` is the timeline zoom remembered for it.
    pub fn open(
        id: TabId,
        project: Project,
        lang: Lang,
        zoom: Option<f32>,
    ) -> Result<Rc<ProjectTab>, String> {
        let root = project.root().to_path_buf();
        let (page, ui) = build_page()?;
        let (timeline, timeline_view) = timeline_widget::new(lang, move |action| {
            with_tab(id, |tab| tab.timeline_action(action));
        });
        page.find("timeline_host")
            .ok_or("project_tab.xml has no timeline_host")?
            .add_child(timeline_view);
        if let Some(zoom) = zoom {
            timeline.set_zoom(zoom);
        }
        let post: Poster = Arc::new(task::post);
        let worker = ProjectWorker::spawn(project, post);
        let watch = watcher::watch(&root, move || {
            task::post(move || with_tab(id, |tab| tab.refresh()));
        });
        let watch = match watch {
            Ok(watch) => Some(watch),
            Err(error) => {
                eprintln!("{} cannot be watched: {error}", root.display());
                None
            }
        };
        Ok(Rc::new(ProjectTab {
            id,
            root,
            page,
            ui,
            live: Some(Live {
                worker,
                timeline,
                _watch: watch,
            }),
            state: RefCell::new(TabState::new()),
        }))
    }

    /// A tab for a folder that does not exist: a plain message and "Remove from list".
    pub fn missing(id: TabId, root: PathBuf) -> Result<Rc<ProjectTab>, String> {
        let (page, ui) = build_page()?;
        let find = |name: &str| {
            page.find(name)
                .ok_or_else(|| format!("project_tab.xml has no widget named {name}"))
        };
        find("content")?.set_visible(false);
        find("missing")?.set_visible(true);
        find("missing.path")?.set_label_text(&root.to_string_lossy());
        Ok(Rc::new(ProjectTab {
            id,
            root,
            page,
            ui,
            live: None,
            state: RefCell::new(TabState::new()),
        }))
    }

    /// Whether the tab shows a folder that was not found.
    pub fn is_missing(&self) -> bool {
        self.live.is_none()
    }

    /// The tab's title: the folder's name.
    pub fn title(&self) -> String {
        folder_name(&self.root)
    }

    /// The timeline's zoom, to remember for next time. `None` for a missing folder.
    pub fn zoom(&self) -> Option<f32> {
        self.live.as_ref().map(|live| live.timeline.zoom())
    }

    /// Which of the tab's buttons `sender` is.
    pub fn button(&self, sender: Widget) -> Option<TabButton> {
        let ui = &self.ui;
        [
            (ui.jump, TabButton::JumpToNow),
            (ui.check_again, TabButton::CheckAgain),
            (ui.remove, TabButton::Remove),
        ]
        .into_iter()
        .find_map(|(widget, button)| (widget == sender).then_some(button))
    }

    pub fn jump_to_now(&self) {
        if let Some(live) = &self.live {
            live.timeline.jump_to_end();
        }
    }

    /// Removes the recovery entries that are too old to keep, in the background. It is only
    /// housekeeping: a failure leaves them for next time.
    pub fn prune_recovery(&self) {
        let Some(live) = &self.live else { return };
        let now = chrono::Utc::now().timestamp();
        live.worker.submit(move |p| {
            if let Err(error) = prune_recovery(p, now) {
                eprintln!("old recovery entries were not removed: {error}");
            }
            Box::new(|| {})
        });
    }

    /// Reads the project's state again in the background, with the history when the latest
    /// save changed, and shows it. Called by the folder watcher, when the window comes back to
    /// the front, by "Check again" and after every operation.
    pub fn refresh(&self) {
        let Some(live) = &self.live else { return };
        let known = self.state.borrow().history_head.clone();
        let id = self.id;
        live.worker.submit_refresh(move |p| {
            let snap = snapshot(p);
            let page =
                (known.as_ref() != Some(&snap.head)).then(|| timeline_page(p, 0, HISTORY_PAGE));
            Box::new(move || with_tab(id, |tab| tab.apply_refresh(snap, page)))
        });
    }

    /// Shows a refresh's result. `page` is the newest page of history, read when the worker saw
    /// another latest save than the one the timeline shows.
    fn apply_refresh(
        &self,
        snap: ProjectSnapshot,
        page: Option<Result<Vec<SaveState>, CoreError>>,
    ) {
        let Some(live) = &self.live else { return };
        let unsaved = snap.changes.len();
        let shows_head = self.state.borrow().history_head.as_ref() == Some(&snap.head);
        match page {
            Some(page) if !shows_head => self.show_history(live, &snap, page),
            _ => {
                live.timeline.set_unsaved(unsaved, snap.other_timelines);
                if !shows_head {
                    // The worker compared with a save the timeline no longer shows (another
                    // refresh landed in between): read again, now against the right one.
                    self.refresh();
                }
            }
        }
        self.show_condition(&snap.condition);
        self.ui.changes_count.set_label_text(&if unsaved == 0 {
            tr("timeline.nothing_new")
        } else {
            tr_plural("changes.unsaved", count(unsaved))
        });
        self.show_count();
    }

    /// Gives the timeline the newest page of a new history, selecting "Now" and showing the end.
    fn show_history(
        &self,
        live: &Live,
        snap: &ProjectSnapshot,
        page: Result<Vec<SaveState>, CoreError>,
    ) {
        let saves = page.unwrap_or_else(|error| {
            // A project this damaged also shows the "cannot be read" banner.
            eprintln!(
                "the history of {} could not be read: {error}",
                self.root.display()
            );
            Vec::new()
        });
        let more_available = saves.len() == HISTORY_PAGE;
        let (was_loaded, was_selected) = {
            let mut state = self.state.borrow_mut();
            let was_loaded = state.history_head.replace(snap.head.clone()).is_some();
            state.more_available = more_available;
            state.data_generation += 1;
            (was_loaded, state.selection != Selection::Now)
        };
        live.timeline.set_data(TimelineData {
            // `timeline_page` gives the newest first; the timeline wants the oldest first.
            saves: saves.into_iter().rev().collect(),
            more_available,
            unsaved: snap.changes.len(),
            selection: Selection::Now,
            other_timelines: snap.other_timelines,
        });
        // The first data scrolls to the end by itself.
        if was_loaded {
            live.timeline.jump_to_end();
        }
        // The stations moved, so what was selected is gone.
        if was_selected {
            self.on_select(Selection::Now);
        }
    }

    fn timeline_action(&self, action: TimelineAction) {
        match action {
            TimelineAction::Select(selection) => self.on_select(selection),
            TimelineAction::RequestOlder => self.load_older(),
            // The timeline already scrolled and repainted itself.
            TimelineAction::JumpToEnd | TimelineAction::Redraw => {}
        }
    }

    /// The user selected a station on the timeline. Task 16 enters or leaves peek here.
    fn on_select(&self, selection: Selection) {
        self.state.borrow_mut().selection = selection;
    }

    /// Loads the page of saves before the oldest one loaded.
    fn load_older(&self) {
        let Some(live) = &self.live else { return };
        let loaded = live.timeline.save_count();
        let generation = self.state.borrow().data_generation;
        let id = self.id;
        live.worker.submit(move |p| {
            let page = timeline_page(p, loaded, HISTORY_PAGE);
            Box::new(move || with_tab(id, |tab| tab.apply_older(generation, page)))
        });
    }

    fn apply_older(&self, generation: u64, page: Result<Vec<SaveState>, CoreError>) {
        let Some(live) = &self.live else { return };
        if generation != self.state.borrow().data_generation {
            // Asked for before the timeline got new data: it would add the wrong saves, and
            // the timeline asks again by itself if it still needs older saves.
            return;
        }
        let (page, more_available) = match page {
            Ok(page) => {
                let more = page.len() == HISTORY_PAGE;
                (page, more)
            }
            Err(error) => {
                // Stop asking: the next history change starts over.
                eprintln!(
                    "older saves of {} could not be read: {error}",
                    self.root.display()
                );
                (Vec::new(), false)
            }
        };
        self.state.borrow_mut().more_available = more_available;
        live.timeline.prepend_older(page, more_available);
        self.show_count();
    }

    /// "1500 saves", "No saves yet", or "More than 500 saves" while older ones are not loaded.
    fn show_count(&self) {
        let Some(live) = &self.live else { return };
        let state = self.state.borrow();
        if state.history_head.is_none() {
            return;
        }
        let n = live.timeline.save_count();
        let text = if state.more_available {
            tr_fmt("timeline.count_more", &[&n.to_string()])
        } else if n == 0 {
            tr("timeline.count_none")
        } else {
            tr_plural("timeline.count", count(n))
        };
        self.ui.count.set_label_text(&text);
    }

    /// Shows or hides the read-only banner, and turns off what could make things worse.
    fn show_condition(&self, condition: &RepoCondition) {
        let ui = &self.ui;
        let keys = banner_keys(condition);
        let read_only = !keys.is_empty();
        let explanation: Vec<String> = keys.iter().map(|key| tr(key)).collect();
        let explanation = explanation.join("\n");
        ui.banner_text.set_label_text(&explanation);
        ui.banner.set_visible(read_only);
        match condition {
            RepoCondition::Broken { details } => {
                ui.details_text.set_label_text(details);
                ui.details.set_visible(true);
            }
            _ => ui.details.set_visible(false),
        }
        let why = read_only.then_some(explanation.as_str());
        for button in [ui.quick_save, ui.throw_away] {
            button.set_enabled(!read_only);
            button.set_tooltip(why);
        }
    }
}

impl TabState {
    fn new() -> TabState {
        TabState {
            history_head: None,
            more_available: false,
            data_generation: 0,
            selection: Selection::Now,
        }
    }
}

/// A count for `tr_plural`.
fn count(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// A new page from `project_tab.xml`, and its widgets.
fn build_page() -> Result<(Widget, TabWidgets), String> {
    let page = widget_from_xml(&ui_files::source("project_tab.xml"))?;
    let find = |name: &str| {
        page.find(name)
            .ok_or_else(|| format!("project_tab.xml has no widget named {name}"))
    };
    let ui = TabWidgets {
        count: find("count")?,
        jump: find("jump")?,
        banner: find("banner")?,
        banner_text: find("banner.text")?,
        check_again: find("banner.check")?,
        details: find("banner.details")?,
        details_text: find("banner.details_text")?,
        changes_count: find("changes_count")?,
        quick_save: find("quick_save")?,
        throw_away: find("throw_away")?,
        remove: find("missing.remove")?,
    };
    Ok((page, ui))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_folder_spelled_differently_activates_its_tab() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let other = tempfile::tempdir().unwrap();
        let mut tabs = TabsState::default();

        assert_eq!(tabs.open(root.clone()), TabOp::Added(0));
        assert_eq!(tabs.open(other.path().to_path_buf()), TabOp::Added(1));
        assert_eq!(tabs.active(), Some(1));

        let upper = PathBuf::from(root.to_string_lossy().to_uppercase());
        assert_eq!(tabs.open(upper), TabOp::Activated(0));
        assert_eq!(tabs.active(), Some(0));
        let mut trailing = root.clone().into_os_string();
        trailing.push("\\");
        assert_eq!(tabs.open(PathBuf::from(trailing)), TabOp::Activated(0));
        assert_eq!(tabs.roots(), [root, other.path().to_path_buf()]);
    }

    #[test]
    fn a_folder_that_does_not_exist_is_still_matched() {
        let mut tabs = TabsState::default();
        assert_eq!(
            tabs.open(PathBuf::from(r"C:\No\Such\Folder")),
            TabOp::Added(0)
        );
        assert_eq!(
            tabs.open(PathBuf::from(r"c:\no\such\folder\")),
            TabOp::Activated(0)
        );
    }

    #[test]
    fn closing_updates_the_active_tab() {
        let mut tabs = TabsState::default();
        for name in ["a", "b", "c", "d"] {
            tabs.open(PathBuf::from(format!(r"C:\Missing\{name}")));
        }
        assert_eq!(tabs.active(), Some(3));

        // Closing a tab before the active one keeps the same tab active.
        tabs.activate(2);
        tabs.close(0);
        assert_eq!(tabs.active(), Some(1));
        assert_eq!(tabs.roots()[1], PathBuf::from(r"C:\Missing\c"));

        // Closing the active tab activates the one that takes its place...
        tabs.close(1);
        assert_eq!(tabs.active(), Some(1));
        assert_eq!(tabs.roots()[1], PathBuf::from(r"C:\Missing\d"));
        // ...or the one before it when it was the last.
        tabs.close(1);
        assert_eq!(tabs.active(), Some(0));

        // Closing a tab after the active one changes nothing.
        tabs.open(PathBuf::from(r"C:\Missing\e"));
        tabs.activate(0);
        tabs.close(1);
        assert_eq!(tabs.active(), Some(0));

        tabs.close(0);
        assert_eq!(tabs.active(), None);
        assert!(tabs.roots().is_empty());
        // Out of range: nothing happens.
        tabs.close(5);
        tabs.activate(5);
        assert_eq!(tabs.active(), None);
    }
}
