//! The open project tabs: which folders are open and which one is active ([`TabsState`]), and
//! what one tab shows ([`ProjectTab`]).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use savestates_core::{
    Change, ChangeKind, CoreError, HISTORY_PAGE, Project, ProjectSnapshot, RepoCondition, SaveId,
    SaveState, Scope, Side, changes_in_save, files_at_save, prune_recovery, snapshot,
    timeline_page,
};
use savestates_i18n::Lang;
use savestates_ui_raven::{Widget, Window, task, tr, tr_fmt, tr_plural, widget_from_xml};

use crate::app::with_tab;
use crate::banner::banner_keys;
use crate::bring_back_ui::{self, BringBackButton, BringBackRow, file_source, has_unsaved_changes};
use crate::changes_panel::ChangesPanel;
use crate::diffpane::widget::{self as diff_widget, DiffPane, PaneInput};
use crate::diffpane::{self, CompareTarget};
use crate::peek::{PeekEvent, PeekTab, PeekView, Peeked};
use crate::start_screen::folder_name;
use crate::timeline::model::{Selection, TimelineAction, TimelineData};
use crate::timeline::widget::{self as timeline_widget, TimelineWidget};
use crate::watcher::{self, WatchHandle};
use crate::worker::{Poster, ProjectWorker};
use crate::{tr_key, ui_files};

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

/// Whether `a` and `b`, both resolved already, name the same folder. Does not ask the file
/// system (see [`spelling_key`]).
pub fn same_spelling(a: &Path, b: &Path) -> bool {
    spelling_key(a) == spelling_key(b)
}

/// Tells the tabs apart in results that come back from other threads.
pub type TabId = u64;

/// A button of a tab that the app reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabButton {
    JumpToNow,
    /// "Quick Save", under the unsaved changes.
    QuickSave,
    CheckAgain,
    /// "Remove from list", in a tab whose folder is gone.
    Remove,
    /// "Back to Now", in the peek banner.
    BackToNow,
    /// "Bring back everything", in the peek banner.
    BringBackEverything,
    /// A bring-back button under a file of peek's "What changed".
    BringBack(BringBackButton),
    /// "Bring back this file", under a file of peek's "All files".
    BringBackThisFile,
    /// "Throw away changes", under the unsaved changes.
    ThrowAway,
    /// "Throw away changes to this file", in the context menu of the unsaved changes.
    ThrowAwayFile,
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
    /// The unsaved changes, their count and the Quick Save and Throw away buttons: what the
    /// left column shows while "Now" is selected.
    now_side: Widget,
    changes_count: Widget,
    quick_save: Widget,
    throw_away: Widget,
    /// The path of the file in the comparison view.
    compare_file: Widget,
    remove: Widget,
}

/// What a tab whose folder is a project has.
struct Live {
    worker: ProjectWorker,
    timeline: TimelineWidget,
    /// Stops watching the folder when the tab closes. `None` when it could not be watched: the
    /// tab then refreshes only when the window comes back to the front or after an operation.
    _watch: Option<WatchHandle>,
    /// The language of the dates.
    lang: Cell<Lang>,
    /// The unsaved changes.
    changes: ChangesPanel,
    /// "Throw away changes to this file", in their context menu.
    throw_away_file: Widget,
    peek: PeekView,
    pane: DiffPane,
    bring_back: BringBackRow,
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
    /// The latest save, as the last refresh found it.
    head: Option<SaveId>,
    /// The project can only be looked at: nothing may be brought back.
    read_only: bool,
    /// The project's condition and how many unsaved changes it has, as the last refresh
    /// showed them (`None` before the first one).
    shown: Option<(RepoCondition, usize)>,
    /// What the comparison view shows, and whether the user asked to see it although it is
    /// large ("Show anyway").
    compare: Option<(CompareTarget, bool)>,
    /// Counts the comparisons asked for: the result of an older one is dropped.
    compare_generation: u64,
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
    /// A tab for `project` in `window`: its page, timeline, worker thread and folder watcher.
    /// Nothing is loaded until [`ProjectTab::refresh`]. `day_px` is the timeline's zoom
    /// remembered for it (how wide a day is, in pixels).
    pub fn open(
        id: TabId,
        project: Project,
        lang: Lang,
        day_px: Option<f64>,
        window: Window,
    ) -> Result<Rc<ProjectTab>, String> {
        let root = project.root().to_path_buf();
        let (page, ui) = build_page()?;
        let (timeline, timeline_view) = timeline_widget::new(lang, move |action| {
            with_tab(id, |tab| tab.timeline_action(action));
        });
        page.find("timeline_host")
            .ok_or("project_tab.xml has no timeline_host")?
            .add_child(timeline_view);
        if let Some(day_px) = day_px {
            timeline.set_day_px(day_px);
        }
        let find = |name: &str| {
            page.find(name)
                .ok_or_else(|| format!("project_tab.xml has no widget named {name}"))
        };
        let changes = ChangesPanel::new(find("changes.list_host")?, move |change| {
            with_tab(id, |tab| tab.now_change_selected(&change));
        });
        let menu = widget_from_xml(&ui_files::source("menu_changes.xml"))?;
        let throw_away_file = menu
            .find("changes.throw_away_file")
            .ok_or("menu_changes.xml has no changes.throw_away_file")?;
        changes.set_context_menu(window, menu);
        let peek = PeekView::new(page, id)?;
        let (pane, pane_view) = diff_widget::new(move || with_tab(id, |tab| tab.show_anyway()));
        find("compare_host")?.add_child(pane_view);
        let bring_back = BringBackRow::new(page)?;
        let post: Poster = Arc::new(task::post);
        let worker = ProjectWorker::spawn(project, post);
        let watch = watcher::watch(&root, move || {
            task::post(move || with_tab(id, |tab| tab.refresh()));
        });
        let watch = match watch {
            Ok(watch) => Some(watch),
            Err(error) => {
                log!("{} cannot be watched: {error}", root.display());
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
                lang: Cell::new(lang),
                changes,
                throw_away_file,
                peek,
                pane,
                bring_back,
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

    /// The timeline's zoom (how wide a day is, in pixels), to remember for next time. `None` for
    /// a missing folder.
    pub fn day_px(&self) -> Option<f64> {
        self.live.as_ref().map(|live| live.timeline.day_px())
    }

    /// Which of the tab's buttons `sender` is.
    pub fn button(&self, sender: Widget) -> Option<TabButton> {
        let ui = &self.ui;
        let fixed = [
            (ui.jump, TabButton::JumpToNow),
            (ui.quick_save, TabButton::QuickSave),
            (ui.check_again, TabButton::CheckAgain),
            (ui.remove, TabButton::Remove),
            (ui.throw_away, TabButton::ThrowAway),
        ]
        .into_iter()
        .find_map(|(widget, button)| (widget == sender).then_some(button));
        if fixed.is_some() {
            return fixed;
        }
        let live = self.live.as_ref()?;
        if sender == live.throw_away_file {
            return Some(TabButton::ThrowAwayFile);
        }
        if sender == live.peek.back {
            return Some(TabButton::BackToNow);
        }
        if sender == live.peek.everything {
            return Some(TabButton::BringBackEverything);
        }
        Some(match live.bring_back.which(sender)? {
            Some(button) => TabButton::BringBack(button),
            None => TabButton::BringBackThisFile,
        })
    }

    /// Runs `job` on the project's thread (see [`ProjectWorker::submit`]); nothing for a folder
    /// that is gone.
    pub fn submit(&self, job: impl FnOnce(&Project) -> Box<dyn FnOnce() + Send> + Send + 'static) {
        if let Some(live) = &self.live {
            live.worker.submit(job);
        }
    }

    /// Whether a Quick Save can start: a project (not a folder that is gone) that can be saved
    /// to, as the last refresh found it.
    pub fn can_save(&self) -> bool {
        self.live.is_some() && !self.state.borrow().read_only
    }

    /// What "Throw away" would throw away now: everything, or with `file` the selected unsaved
    /// change. `None` when nothing may be thrown away: a project that can only be looked at, or
    /// without saves to go back to, nothing to throw away, or while peeking at a save.
    pub fn throw_away_scope(&self, file: bool) -> Option<Scope> {
        let live = self.live.as_ref()?;
        let state = self.state.borrow();
        if state.read_only || state.head.is_none() || live.peek.peeked_id().is_some() {
            return None;
        }
        if file {
            return live
                .changes
                .selected()
                .map(|change| Scope::File(change.path));
        }
        (!live.changes.changes().is_empty()).then_some(Scope::All)
    }

    /// Why nothing may be brought back from the Recovery list now (the project can only be
    /// looked at), or `None` when it may.
    pub fn restore_off(&self) -> Option<String> {
        let state = self.state.borrow();
        let (condition, _) = state.shown.as_ref()?;
        let why: Vec<String> = banner_keys(condition).iter().map(|key| tr(key)).collect();
        (!why.is_empty()).then(|| why.join("\n"))
    }

    /// The language of the tab's dates (`None` for a folder that is gone).
    pub fn lang(&self) -> Option<Lang> {
        self.live.as_ref().map(|live| live.lang.get())
    }

    /// The language just changed to `lang`: shows every text set in code again, in it (Raven
    /// already changed the texts of the layout's `@keys`).
    pub fn relabel(&self, lang: Lang) {
        let Some(live) = &self.live else { return };
        live.lang.set(lang);
        live.timeline.set_lang(lang);
        live.peek.relabel(lang);
        live.changes.relabel();
        let shown = self.state.borrow().shown.clone();
        if let Some((condition, unsaved)) = shown {
            self.show_condition(&condition, unsaved);
            self.show_changes_count(unsaved);
        }
        self.show_count();
        // The comparison's titles, or the note shown in its place.
        if live.peek.peeked_id().is_some() {
            self.show_peek_tab();
        } else {
            self.compare_now(live.changes.selected());
        }
    }

    /// Shows the unsaved change of the file at `path` and compares it with the latest save
    /// ("Cancel and fix it" in the warnings before a save), leaving peek mode first.
    pub fn select_file(&self, path: &str) {
        let Some(live) = &self.live else { return };
        self.back_to_now();
        if let Some(change) = live.changes.select_path(path) {
            self.compare_now(Some(change));
        }
    }

    /// The app just made `save`, whose previous save is `parent`: it grows into the timeline.
    ///
    /// Only when the timeline shows the history `save` continues; otherwise the refresh that
    /// follows reads the history again. The timeline's history is now `save`'s, so that refresh
    /// leaves the timeline (and its animation) alone.
    pub fn show_new_save(&self, save: SaveState, parent: Option<SaveId>) {
        let Some(live) = &self.live else { return };
        {
            let mut state = self.state.borrow_mut();
            if state.history_head != Some(parent) {
                return;
            }
            state.history_head = Some(Some(save.id.clone()));
            state.head = Some(save.id.clone());
        }
        live.timeline.append_saved(save);
        self.show_count();
    }

    pub fn jump_to_now(&self) {
        if let Some(live) = &self.live {
            live.timeline.jump_to_end();
        }
    }

    /// Esc in the window: the timeline closes a group's list, or ends its temporary zoom.
    pub fn escape(&self) {
        if let Some(live) = &self.live {
            live.timeline.escape();
        }
    }

    /// Esc with the pointer over the timeline (from the window's key hook): it closes a group's
    /// list, or ends its temporary zoom. Returns whether it did; false also when the pointer is
    /// elsewhere.
    pub fn escape_under_pointer(&self) -> bool {
        self.live
            .as_ref()
            .is_some_and(|live| live.timeline.escape_under_pointer())
    }

    /// Removes the recovery entries that are too old to keep, in the background. It is only
    /// housekeeping: a failure leaves them for next time.
    pub fn prune_recovery(&self) {
        let Some(live) = &self.live else { return };
        let now = chrono::Utc::now().timestamp();
        live.worker.submit(move |p| {
            if let Err(error) = prune_recovery(p, now) {
                log!("old recovery entries were not removed: {error}");
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
        self.state.borrow_mut().head.clone_from(&snap.head);
        let stale = needs_history(&self.state.borrow().history_head, &snap.head);
        match page {
            Some(page) if stale => self.show_history(live, &snap, page),
            _ => {
                live.timeline.set_unsaved(unsaved, snap.other_timelines);
                // New data moves "Agora" to the current time by itself; this does it otherwise.
                live.timeline.tick_clock();
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
        self.state.borrow_mut().shown = Some((snap.condition.clone(), unsaved));
        self.show_condition(&snap.condition, unsaved);
        self.show_changes_count(unsaved);
        self.show_count();
        // The files on disk changed: what compares with them is compared again.
        let selected = live.changes.set_changes(snap.changes);
        match (live.peek.peeked_id(), live.peek.tab()) {
            (None, _) => self.compare_now(selected),
            (Some(_), PeekTab::AllFiles) => self.show_peek_tab(),
            (Some(_), PeekTab::Changed) => {}
        }
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
                log!(
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
            // The timeline animates its view and lists a group's saves itself (a save picked
            // from the list comes as `Select`): these never reach the tab.
            TimelineAction::Animate { .. } | TimelineAction::OpenGroupList(_) => {}
        }
    }

    /// A station was selected on the timeline: a save opens peek mode, "Now" leaves it.
    fn on_select(&self, selection: Selection) {
        self.state.borrow_mut().selection = selection;
        match selection {
            Selection::Station(i) => self.enter_peek(i),
            Selection::Now => self.leave_peek(),
        }
    }

    /// "Back to Now" in the peek banner: as if "Now" was clicked on the timeline.
    pub fn back_to_now(&self) {
        if let Some(live) = &self.live {
            live.timeline.set_selection(Selection::Now);
        }
        self.on_select(Selection::Now);
    }

    /// Peeks at the save at index `i` of the timeline's loaded saves (oldest first), and reads
    /// what it changed and every file in it.
    fn enter_peek(&self, i: usize) {
        let Some(live) = &self.live else { return };
        let Some(save) = live.timeline.save_at(i) else {
            return;
        };
        if live.peek.peeked_id().as_ref() == Some(&save.id) {
            return;
        }
        let before_name = i
            .checked_sub(1)
            .and_then(|before| live.timeline.save_at(before))
            .map(|before| before.name);
        let save_id = save.id.clone();
        live.peek.enter(save, before_name, live.lang.get());
        self.ui.now_side.set_visible(false);
        live.bring_back.hide();
        self.show_note("");
        let id = self.id;
        live.worker.submit(move |p| {
            let read = changes_in_save(p, &save_id)
                .and_then(|changes| Ok((changes, files_at_save(p, &save_id)?)));
            Box::new(move || with_tab(id, |tab| tab.peek_loaded(&save_id, read)))
        });
    }

    /// What the save `id` changed and its files were read (or could not be).
    fn peek_loaded(&self, id: &SaveId, read: Result<(Vec<Change>, Vec<String>), CoreError>) {
        let Some(live) = &self.live else { return };
        if live.peek.peeked_id().as_ref() != Some(id) {
            return;
        }
        match read {
            Ok((changes, files)) => {
                let selected = live.peek.loaded(id, changes, files);
                if live.peek.tab() == PeekTab::Changed {
                    self.show_peek_change(selected.as_ref());
                }
            }
            Err(error) => {
                log!("the save {} could not be read: {error}", id.0);
                self.show_note(&tr("peek.unreadable"));
            }
        }
    }

    /// Leaves peek mode, back to the unsaved changes.
    fn leave_peek(&self) {
        let Some(live) = &self.live else { return };
        if live.peek.peeked_id().is_none() {
            return;
        }
        live.peek.leave();
        live.bring_back.hide();
        self.ui.now_side.set_visible(true);
        self.compare_now(live.changes.selected());
    }

    /// What the user did in peek mode.
    pub fn peek_event(&self, event: PeekEvent) {
        let Some(live) = &self.live else { return };
        match event {
            PeekEvent::ChangeSelected(change) => self.show_peek_change(Some(&change)),
            PeekEvent::ItemSelected(item) => {
                live.peek.select_item(item);
                self.show_peek_tab();
            }
            PeekEvent::TabChanged(tab) => {
                live.peek.set_tab(tab);
                self.show_peek_tab();
            }
            PeekEvent::Filter => live.peek.filter(),
        }
    }

    /// Compares what the selection of peek's current tab stands for.
    fn show_peek_tab(&self) {
        let Some(live) = &self.live else { return };
        match live.peek.tab() {
            PeekTab::Changed => self.show_peek_change(live.peek.changes.selected().as_ref()),
            PeekTab::AllFiles => {
                let target = live.peek.with_peeked(|peeked| {
                    let path = peeked.file.clone()?;
                    Some(all_files_target(peeked, path))
                });
                match target.flatten() {
                    Some(target) => {
                        live.bring_back.show_this_file();
                        self.compare(target, false);
                    }
                    None => {
                        live.bring_back.hide();
                        self.show_note(&tr("peek.pick_a_file"));
                    }
                }
            }
        }
    }

    /// Compares a file the peeked save changed with how it was before that save.
    fn show_peek_change(&self, change: Option<&Change>) {
        let Some(live) = &self.live else { return };
        let target = change.and_then(|change| {
            live.peek
                .with_peeked(|peeked| (change.kind.clone(), save_change_target(peeked, change)))
        });
        match target {
            Some((kind, target)) => {
                live.bring_back.show_for(&kind);
                self.compare(target, false);
            }
            None => {
                live.bring_back.hide();
                self.show_note(&tr("peek.no_changes"));
            }
        }
    }

    /// The user selected an unsaved change.
    fn now_change_selected(&self, change: &Change) {
        let Some(live) = &self.live else { return };
        if live.peek.peeked_id().is_none() {
            self.compare_now(Some(change.clone()));
        }
    }

    /// Compares an unsaved change with the latest save, or says that nothing changed.
    fn compare_now(&self, change: Option<Change>) {
        match change {
            Some(change) => self.compare(self.now_target(&change), false),
            None => self.show_note(&tr("timeline.nothing_new")),
        }
    }

    /// The comparison of an unsaved change: the latest save against Now.
    fn now_target(&self, change: &Change) -> CompareTarget {
        let head = self.state.borrow().head.clone();
        let head_name = self.live.as_ref().and_then(|live| {
            let newest = live
                .timeline
                .save_at(live.timeline.save_count().checked_sub(1)?)?;
            (Some(&newest.id) == head.as_ref()).then_some(newest.name)
        });
        CompareTarget {
            before: head.map(|head| (Side::Save(head), path_before(change))),
            after: (Side::Now, change.path.clone()),
            before_title: titled(tr("compare.before"), head_name),
            after_title: titled(tr("compare.after"), Some(tr("timeline.now"))),
            after_missing_key: tr_key("compare.was_deleted"),
        }
    }

    /// Shows `target` in the comparison view once the worker has read and compared it. With
    /// `force`, it is shown whatever its size.
    fn compare(&self, target: CompareTarget, force: bool) {
        let Some(live) = &self.live else { return };
        let (generation, force, same) = self.state.borrow_mut().compare_asked(&target, force);
        self.ui.compare_file.set_label_text(target.path());
        if !same {
            // Not the previous file's comparison under this file's name meanwhile.
            live.pane.show_note("");
        }
        let id = self.id;
        live.worker.submit(move |p| {
            let loaded = diffpane::load(p, &target, force);
            Box::new(move || with_tab(id, |tab| tab.compared(generation, same, loaded)))
        });
    }

    /// A comparison was read. `same`: it compares again the file already shown.
    fn compared(&self, generation: u64, same: bool, loaded: Result<PaneInput, CoreError>) {
        let Some(live) = &self.live else { return };
        if self.state.borrow().compare_generation != generation {
            return;
        }
        match loaded {
            Ok(input) => live.pane.show(input, same),
            Err(error) => {
                log!("a comparison could not be read: {error}");
                live.pane.show_note(&tr("compare.unreadable"));
            }
        }
    }

    /// "Show anyway", on a comparison too large to show by default.
    fn show_anyway(&self) {
        let target = self.state.borrow().compare.clone();
        if let Some((target, _)) = target {
            self.compare(target, true);
        }
    }

    /// Shows a note instead of a comparison.
    fn show_note(&self, text: &str) {
        let Some(live) = &self.live else { return };
        self.state.borrow_mut().compare_dropped();
        self.ui.compare_file.set_label_text("");
        live.pane.show_note(text);
    }

    /// A bring-back button under the peeked file: `Some` for one of "What changed", `None`
    /// for "Bring back this file" of "All files".
    pub fn bring_back(&self, window: Window, button: Option<BringBackButton>) {
        let Some(live) = &self.live else { return };
        let Some(save) = live.peek.peeked_id() else {
            return;
        };
        if self.state.borrow().read_only {
            return;
        }
        let source = match button {
            Some(button) => live
                .peek
                .changes
                .selected()
                .map(|change| file_source(button, &save, &change)),
            None => live
                .peek
                .with_peeked(|peeked| peeked.file.clone())
                .flatten()
                .map(|path| (Side::Save(save), path)),
        };
        let Some((side, path)) = source else { return };
        let ask_first = has_unsaved_changes(&path, &live.changes.changes());
        bring_back_ui::bring_back_file(window, self.id, side, path, ask_first);
    }

    /// "Bring back everything" in the peek banner.
    pub fn bring_back_everything(&self, window: Window) {
        let Some(live) = &self.live else { return };
        let Some(save) = live.peek.peeked_id() else {
            return;
        };
        if self.state.borrow().read_only {
            return;
        }
        let has_unsaved = !live.changes.changes().is_empty();
        bring_back_ui::bring_back_everything(window, self.id, save, has_unsaved);
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
                log!(
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

    /// "3 unsaved changes", or that nothing changed since the last save.
    fn show_changes_count(&self, unsaved: usize) {
        self.ui.changes_count.set_label_text(&if unsaved == 0 {
            tr("timeline.nothing_new")
        } else {
            tr_plural("changes.unsaved", count(unsaved))
        });
    }

    /// Shows or hides the read-only banner, and turns off what could make things worse. Quick
    /// Save and Throw away are off as well when there are no `unsaved` changes, and Throw away
    /// before the first save (there is no save to go back to).
    fn show_condition(&self, condition: &RepoCondition, unsaved: usize) {
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
        self.state.borrow_mut().read_only = read_only;
        let bring_back = self.live.iter().flat_map(|live| {
            std::iter::once(live.peek.everything).chain(live.bring_back.widgets())
        });
        for button in bring_back {
            button.set_enabled(!read_only);
            button.set_tooltip(why);
        }
        let nothing_new = tr("timeline.nothing_new");
        let quick_save_off = why.or((unsaved == 0).then_some(nothing_new.as_str()));
        ui.quick_save.set_enabled(quick_save_off.is_none());
        ui.quick_save.set_tooltip(quick_save_off);
        let no_saves = tr("throw_away.no_saves_yet");
        let has_saves = self.state.borrow().head.is_some();
        let throw_away_off = why
            .or((!has_saves).then_some(no_saves.as_str()))
            .or((unsaved == 0).then_some(nothing_new.as_str()));
        let throw_away_file = self.live.as_ref().map(|live| live.throw_away_file);
        for button in std::iter::once(ui.throw_away).chain(throw_away_file) {
            button.set_enabled(throw_away_off.is_none());
            button.set_tooltip(throw_away_off);
        }
    }
}

/// Where a file was before a change: its old name for a rename, else the same path.
fn path_before(change: &Change) -> String {
    match &change.kind {
        ChangeKind::Renamed { from } => from.clone(),
        _ => change.path.clone(),
    }
}

/// A column header: "Antes · Galeria de fotos", or the word alone without a name.
fn titled(word: String, name: Option<String>) -> String {
    match name {
        Some(name) => format!("{word} · {name}"),
        None => word,
    }
}

/// The comparison of a file the peeked save changed: just before that save against it.
fn save_change_target(peeked: &Peeked, change: &Change) -> CompareTarget {
    let id = &peeked.save.id;
    CompareTarget {
        before: Some((Side::Before(id.clone()), path_before(change))),
        after: (Side::Save(id.clone()), change.path.clone()),
        before_title: titled(tr("compare.before"), peeked.before_name.clone()),
        after_title: titled(tr("compare.after"), Some(peeked.save.name.clone())),
        after_missing_key: tr_key("compare.was_deleted"),
    }
}

/// The comparison of a file of the peeked save (from "All files") with Now: what bringing it
/// back would change.
fn all_files_target(peeked: &Peeked, path: String) -> CompareTarget {
    CompareTarget {
        before: Some((Side::Save(peeked.save.id.clone()), path.clone())),
        after: (Side::Now, path),
        before_title: titled(tr("compare.before"), Some(peeked.save.name.clone())),
        after_title: titled(tr("compare.after"), Some(tr("timeline.now"))),
        after_missing_key: tr_key("compare.no_longer_exists"),
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
            head: None,
            read_only: false,
            shown: None,
            compare: None,
            compare_generation: 0,
        }
    }

    /// The comparison of `target` is asked for: returns its number, whether "Show anyway" was
    /// asked for it, and whether it is the one shown already (compared again after a change on
    /// disk). With `force`, it is shown whatever its size from now on.
    fn compare_asked(&mut self, target: &CompareTarget, force: bool) -> (u64, bool, bool) {
        self.compare_generation += 1;
        let same = self
            .compare
            .as_ref()
            .is_some_and(|(shown, _)| shown == target);
        let force = force || (same && self.compare.as_ref().is_some_and(|(_, forced)| *forced));
        self.compare = Some((target.clone(), force));
        (self.compare_generation, force, same)
    }

    /// Nothing is compared any more; a comparison still on its way is dropped.
    fn compare_dropped(&mut self) {
        self.compare_generation += 1;
        self.compare = None;
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
        now_side: find("now_side")?,
        changes_count: find("changes_count")?,
        quick_save: find("quick_save")?,
        throw_away: find("throw_away")?,
        compare_file: find("compare.file")?,
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

    fn now_target(path: &str) -> CompareTarget {
        CompareTarget {
            before: head(1).map(|head| (Side::Save(head), path.to_owned())),
            after: (Side::Now, path.to_owned()),
            before_title: "Antes".to_owned(),
            after_title: "Depois · Agora".to_owned(),
            after_missing_key: "compare.was_deleted",
        }
    }

    #[test]
    fn show_anyway_sticks_to_its_file_and_older_comparisons_are_dropped() {
        let mut state = TabState::new();
        let (a, b) = (now_target("a.txt"), now_target("b.txt"));
        assert_eq!(state.compare_asked(&a, false), (1, false, false));
        // The same file again (it changed on disk): the view is kept.
        assert_eq!(state.compare_asked(&a, false), (2, false, true));
        // "Show anyway" holds for that file from then on...
        assert_eq!(state.compare_asked(&a, true), (3, true, true));
        assert_eq!(state.compare_asked(&a, false), (4, true, true));
        // ...and not for another one.
        assert_eq!(state.compare_asked(&b, false), (5, false, false));
        // A note in place of the comparison drops the one on its way, and forgets the file.
        state.compare_dropped();
        assert_eq!(state.compare_generation, 6);
        assert_eq!(state.compare, None);
        assert_eq!(state.compare_asked(&b, false), (7, false, false));
    }

    #[test]
    fn a_renamed_file_is_compared_with_its_old_name() {
        let renamed = Change {
            path: "css/novo.css".to_owned(),
            kind: ChangeKind::Renamed {
                from: "css/velho.css".to_owned(),
            },
        };
        assert_eq!(path_before(&renamed), "css/velho.css");
        let changed = Change {
            path: "index.html".to_owned(),
            kind: ChangeKind::Changed,
        };
        assert_eq!(path_before(&changed), "index.html");
        assert_eq!(titled("Antes".to_owned(), None), "Antes");
        assert_eq!(
            titled("Antes".to_owned(), Some("Modo escuro".to_owned())),
            "Antes · Modo escuro"
        );
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
