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
    spelling_key(&resolved)
}

/// [`folder_key`] without asking the file system (which can hang on a share that is offline):
/// for paths the app wrote itself, already resolved.
fn spelling_key(path: &Path) -> String {
    let text = path.to_string_lossy();
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

/// The tabs of the last run while they reopen in the background, one by one and in their order.
/// Until every one of them is back, what is saved keeps the ones still on their way, so closing
/// the app early (or a folder that is slow to open) loses none.
#[derive(Debug, Default)]
pub struct RestoreState {
    /// The folders of the last run, in tab order.
    saved: Vec<PathBuf>,
    /// The ones not back yet.
    pending: Vec<PathBuf>,
    /// The one that was active.
    active: Option<PathBuf>,
}

impl RestoreState {
    /// Starts reopening `saved`, of which the one at `active_tab` was active.
    pub fn new(saved: Vec<PathBuf>, active_tab: usize) -> RestoreState {
        RestoreState {
            active: saved.get(active_tab).cloned(),
            pending: saved.clone(),
            saved,
        }
    }

    pub fn in_progress(&self) -> bool {
        !self.pending.is_empty()
    }

    /// `folder` (one of the saved ones, as given) came back, as a tab or not. Returns whether it
    /// is the one that was active.
    pub fn arrived(&mut self, folder: &Path) -> bool {
        if let Some(i) = self.pending.iter().position(|p| p == folder) {
            self.pending.remove(i);
        }
        self.active
            .as_deref()
            .is_some_and(|active| same_spelling(active, folder))
    }

    /// The tabs to save and the index of the active one, given the tabs open now (`open`, with
    /// `active` the active index).
    ///
    /// Once everything is back, these are the open tabs. Before: the saved folders in their
    /// order (the ones on their way, and the ones back while their tab is still open), then the
    /// tabs opened since. The folder active in the last run stays active while it is on its way.
    pub fn tabs_to_save(&self, open: &[PathBuf], active: Option<usize>) -> (Vec<PathBuf>, usize) {
        if !self.in_progress() {
            return (open.to_vec(), active.unwrap_or(0));
        }
        let pending = |folder: &Path| self.pending.iter().any(|p| same_spelling(p, folder));
        let mut tabs: Vec<PathBuf> = Vec::new();
        for folder in &self.saved {
            if pending(folder) {
                tabs.push(folder.clone());
            } else if let Some(root) = open.iter().find(|root| same_spelling(root, folder)) {
                tabs.push(root.clone());
            }
        }
        for root in open {
            if !tabs.iter().any(|tab| same_spelling(tab, root)) {
                tabs.push(root.clone());
            }
        }
        let active_folder = match &self.active {
            Some(folder) if pending(folder) => Some(folder),
            _ => active.and_then(|i| open.get(i)),
        };
        let index = active_folder
            .and_then(|folder| tabs.iter().position(|tab| same_spelling(tab, folder)))
            .unwrap_or(0);
        (tabs, index)
    }
}

fn same_spelling(a: &Path, b: &Path) -> bool {
    spelling_key(a) == spelling_key(b)
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
    /// The last read of the newest page failed. The timeline keeps what it showed, and
    /// `history_head` the save that belongs to, so the next refresh reads again.
    newest_failed: bool,
    /// The last read of an older page failed: the next refresh asks for it again.
    older_failed: bool,
}

/// Whether a refresh that found the project at `head` has to read the newest page of history:
/// the timeline shows the history of another save (`shown`), or none yet.
fn needs_history(shown: &Option<Option<SaveId>>, head: &Option<SaveId>) -> bool {
    shown.as_ref() != Some(head)
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

    /// Reads the project's state again in the background, with the history when the timeline
    /// does not show the latest save's (it changed, or reading it failed before), and shows it.
    /// Called by the folder watcher, when the window comes back to the front, by "Check again"
    /// and after every operation.
    pub fn refresh(&self) {
        let Some(live) = &self.live else { return };
        let shown = self.state.borrow().history_head.clone();
        let id = self.id;
        live.worker.submit_refresh(move |p| {
            let snap = snapshot(p);
            let page = needs_history(&shown, &snap.head).then(|| timeline_page(p, 0, HISTORY_PAGE));
            Box::new(move || with_tab(id, |tab| tab.apply_refresh(snap, page)))
        });
    }

    /// Shows a refresh's result. `page` is the newest page of history, read when the worker saw
    /// that the timeline does not show the latest save's history.
    fn apply_refresh(
        &self,
        snap: ProjectSnapshot,
        page: Option<Result<Vec<SaveState>, CoreError>>,
    ) {
        let Some(live) = &self.live else { return };
        let unsaved = snap.changes.len();
        let stale = needs_history(&self.state.borrow().history_head, &snap.head);
        match page {
            Some(page) if stale => self.show_history(live, &snap, page),
            _ => {
                live.timeline.set_unsaved(unsaved, snap.other_timelines);
                if stale {
                    // The worker compared with a save the timeline no longer shows (another
                    // refresh landed in between): read again, now against the right one.
                    self.refresh();
                }
            }
        }
        // An older page that could not be read last time is asked for again.
        let retry_older = self.state.borrow_mut().take_older_retry();
        if retry_older {
            self.load_older();
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
    /// When it could not be read, the timeline keeps what it shows (with "Now" up to date) and
    /// the next refresh reads again.
    fn show_history(
        &self,
        live: &Live,
        snap: &ProjectSnapshot,
        page: Result<Vec<SaveState>, CoreError>,
    ) {
        let saves = match page {
            Ok(saves) => saves,
            Err(error) => {
                // History objects missing (a synced `.git` folder) or git failing once: the
                // project's condition can still read Normal, so say so in the count instead.
                eprintln!(
                    "the history of {} could not be read: {error}",
                    self.root.display()
                );
                self.state.borrow_mut().newest_page_failed();
                live.timeline
                    .set_unsaved(snap.changes.len(), snap.other_timelines);
                return;
            }
        };
        let (was_loaded, was_selected, more_available) = {
            let mut state = self.state.borrow_mut();
            let was_loaded = state.newest_page_read(&snap.head, saves.len());
            (
                was_loaded,
                state.selection != Selection::Now,
                state.more_available,
            )
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

    /// A page asked for before the timeline got new data is dropped: it would add the wrong
    /// saves, and the timeline asks again by itself if it still needs older ones.
    fn apply_older(&self, generation: u64, page: Result<Vec<SaveState>, CoreError>) {
        let Some(live) = &self.live else { return };
        match page {
            Ok(page) => {
                let read = self
                    .state
                    .borrow_mut()
                    .older_page_read(generation, page.len());
                let Some(more_available) = read else { return };
                live.timeline.prepend_older(page, more_available);
            }
            Err(error) => {
                // The timeline keeps waiting for this page (it asks for no other meanwhile),
                // and the next refresh asks for it again.
                if !self.state.borrow_mut().older_page_failed(generation) {
                    return;
                }
                eprintln!(
                    "older saves of {} could not be read: {error}",
                    self.root.display()
                );
            }
        }
        self.show_count();
    }

    /// "1500 saves", "No saves yet", "More than 500 saves" while older ones are not loaded, or
    /// that the history could not be read.
    fn show_count(&self) {
        let Some(live) = &self.live else { return };
        let state = self.state.borrow();
        let n = live.timeline.save_count();
        let text = if state.read_failed() {
            tr("timeline.history_unreadable")
        } else if state.history_head.is_none() {
            return;
        } else if state.more_available {
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
            newest_failed: false,
            older_failed: false,
        }
    }

    /// The newest page of `head`'s history was read, `len` saves long, and the timeline gets it.
    /// Returns whether it showed a history before.
    fn newest_page_read(&mut self, head: &Option<SaveId>, len: usize) -> bool {
        self.newest_failed = false;
        self.older_failed = false;
        self.more_available = len == HISTORY_PAGE;
        self.data_generation += 1;
        self.history_head.replace(head.clone()).is_some()
    }

    /// Reading the newest page failed. Nothing else changes: the timeline keeps what it shows,
    /// and since that is not `head`'s history the next refresh reads it again.
    fn newest_page_failed(&mut self) {
        self.newest_failed = true;
    }

    /// An older page asked for at `generation` was read, `len` saves long. Returns whether saves
    /// older than it exist, or `None` when the page belongs to data the timeline no longer has.
    fn older_page_read(&mut self, generation: u64, len: usize) -> Option<bool> {
        if generation != self.data_generation {
            return None;
        }
        self.older_failed = false;
        self.more_available = len == HISTORY_PAGE;
        Some(self.more_available)
    }

    /// Reading an older page asked for at `generation` failed: the older saves are still there,
    /// and the next refresh asks again. False when the page belongs to data the timeline no
    /// longer has (then nothing changes).
    fn older_page_failed(&mut self, generation: u64) -> bool {
        if generation != self.data_generation {
            return false;
        }
        self.older_failed = true;
        true
    }

    /// Whether an older page failed and is to be asked for again now (once).
    fn take_older_retry(&mut self) -> bool {
        std::mem::take(&mut self.older_failed)
    }

    /// Whether the last read of the history failed.
    fn read_failed(&self) -> bool {
        self.newest_failed || self.older_failed
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

    fn folders(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| PathBuf::from(format!(r"C:\Projects\{name}")))
            .collect()
    }

    #[test]
    fn while_tabs_reopen_the_saved_ones_are_kept() {
        let saved = folders(&["a", "b", "c", "e"]);
        let mut restore = RestoreState::new(saved.clone(), 2);
        assert!(restore.in_progress());

        // Closing the app before any tab is back keeps the last run's tabs and active one.
        assert_eq!(restore.tabs_to_save(&[], None), (saved.clone(), 2));

        // "a" is back; "b" did not reopen (not a project any more, say); "c" and "e" are on
        // their way. The user opened "d" meanwhile, which goes after them.
        assert!(!restore.arrived(&saved[0]));
        assert!(!restore.arrived(&saved[1]));
        let open = vec![saved[0].clone(), PathBuf::from(r"C:\Projects\d")];
        assert_eq!(
            restore.tabs_to_save(&open, Some(1)),
            (
                vec![
                    saved[0].clone(),
                    saved[2].clone(),
                    saved[3].clone(),
                    open[1].clone()
                ],
                1
            ),
            "the active folder of the last run stays active while it is on its way"
        );

        // A tab closed after it came back is not kept; once the active folder is back, the
        // active tab is the one active now. The same folder spelled differently is one tab.
        assert!(restore.arrived(&saved[2]));
        let open = vec![
            PathBuf::from(r"c:\projects\D\"),
            PathBuf::from(r"C:\Projects\c"),
        ];
        assert_eq!(
            restore.tabs_to_save(&open, Some(0)),
            (vec![open[1].clone(), saved[3].clone(), open[0].clone()], 2)
        );

        // Once everything is back, what is saved is just the open tabs.
        assert!(!restore.arrived(&saved[3]));
        assert!(!restore.in_progress());
        assert_eq!(restore.tabs_to_save(&open, Some(1)), (open.clone(), 1));
        assert_eq!(restore.tabs_to_save(&[], None), (Vec::new(), 0));
    }

    #[test]
    fn nothing_to_restore_saves_the_open_tabs() {
        let restore = RestoreState::default();
        assert!(!restore.in_progress());
        let open = folders(&["x"]);
        assert_eq!(restore.tabs_to_save(&open, Some(0)), (open.clone(), 0));
        // An active index past the end of the last run's tabs names no folder.
        let mut restore = RestoreState::new(folders(&["a"]), 7);
        assert!(!restore.arrived(&folders(&["a"])[0]));
        assert!(!restore.in_progress());
    }

    fn head(n: u8) -> Option<SaveId> {
        Some(SaveId(format!("{n:040x}")))
    }

    #[test]
    fn a_failed_history_read_is_read_again_by_the_next_refresh() {
        let mut state = TabState::new();
        assert!(needs_history(&state.history_head, &head(1)));

        // The first read fails: nothing is recorded as shown, so the next refresh reads again.
        state.newest_page_failed();
        assert!(state.read_failed());
        assert!(needs_history(&state.history_head, &head(1)));
        assert_eq!(state.data_generation, 0);

        // It works: the history of that save is shown, and refreshes at it read nothing.
        assert!(!state.newest_page_read(&head(1), HISTORY_PAGE));
        assert!(!state.read_failed());
        assert!(state.more_available);
        assert_eq!(state.data_generation, 1);
        assert!(!needs_history(&state.history_head, &head(1)));

        // A new save whose history cannot be read keeps the old history on screen, and the
        // save it belongs to, so the read is tried again.
        state.newest_page_failed();
        assert!(needs_history(&state.history_head, &head(2)));
        assert!(state.more_available);
        assert_eq!(state.data_generation, 1);
        assert!(state.newest_page_read(&head(2), 3));
        assert!(!state.more_available);
        assert!(!needs_history(&state.history_head, &head(2)));

        // A project without saves shows that, and is not read again.
        let mut empty = TabState::new();
        empty.newest_page_read(&None, 0);
        assert!(!needs_history(&empty.history_head, &None));
    }

    #[test]
    fn a_failed_older_page_is_asked_for_again_once() {
        let mut state = TabState::new();
        state.newest_page_read(&head(1), HISTORY_PAGE);
        let generation = state.data_generation;

        // The read fails: more saves are still there to load, and the next refresh asks again.
        assert!(state.older_page_failed(generation));
        assert!(state.more_available);
        assert!(state.read_failed());
        assert!(state.take_older_retry());
        assert!(!state.take_older_retry(), "asked again once");

        // The retry works.
        assert_eq!(state.older_page_read(generation, 120), Some(false));
        assert!(!state.more_available);
        assert!(!state.read_failed());

        // A page asked for before the timeline got new data is dropped, failed or not, and a
        // new history forgets a pending retry.
        assert!(state.older_page_failed(generation));
        state.newest_page_read(&head(2), HISTORY_PAGE);
        assert!(!state.take_older_retry());
        assert_eq!(state.older_page_read(generation, HISTORY_PAGE), None);
        assert!(!state.older_page_failed(generation));
        assert!(!state.take_older_retry());
    }
}
