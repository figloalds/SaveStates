//! The open project tabs: which folders are open and which one is active ([`TabsState`]), and
//! what one tab shows ([`ProjectTab`]).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use savestates_core::{
    Change, ChangeKind, CoreError, GraphSave, HISTORY_PAGE, Project, ProjectSnapshot,
    RepoCondition, SaveId, Scope, Side, Timeline, TimelinesView, assign_lanes, changes_in_save,
    files_at_save, prune_recovery, read_older_saves, read_timelines, snapshot,
    sweep_empty_timelines,
};
use savestates_i18n::Lang;
use savestates_ui_raven::{Widget, Window, task, tr, tr_fmt, tr_plural, widget_from_xml};

use crate::app::with_tab;
use crate::banner::{banner_keys, offers_load_here};
use crate::bring_back_ui::{self, BringBackButton, BringBackRow, file_source, has_unsaved_changes};
use crate::changes_panel::ChangesPanel;
use crate::diffpane::widget::{self as diff_widget, DiffPane, PaneInput};
use crate::diffpane::{self, CompareTarget};
use crate::peek::{LoadButton, PeekEvent, PeekTab, PeekView, Peeked, load_button, load_tip};
use crate::start_screen::folder_name;
use crate::timeline::data::{LaneNames, merge_older, timeline_data, timeline_label};
use crate::timeline::model::{LaneData, Selection, TimelineAction, TimelineData};
use crate::timeline::widget::{self as timeline_widget, TimelineWidget};
use crate::watcher::{self, WatchHandle};
use crate::worker::{Poster, ProjectWorker, when_normal};
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
    /// "Carregar este save", in the peek banner.
    Load,
    /// "Carregar este save", in the read-only banner of a project on a bare save.
    LoadHere,
}

/// The widgets of a tab that change while it is open (`project_tab.xml`).
struct TabWidgets {
    count: Widget,
    jump: Widget,
    banner: Widget,
    banner_text: Widget,
    check_again: Widget,
    /// "Carregar este save", in the read-only banner.
    load_here: Widget,
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
    /// Every timeline as last read, with every save loaded so far (the newest page, and the
    /// older pages added since). `None` until the first read.
    timelines: Option<TimelinesView>,
    /// The timelines key (see [`ProjectSnapshot::timelines_key`]) of the project when
    /// `timelines` was read. `None` until the first read.
    shown_key: Option<String>,
    /// The selected save, by id: it stays selected across reloads while it is loaded.
    selected: Option<SaveId>,
    /// The save just made: the next data that holds it grows it in.
    grow: Option<SaveId>,
    /// The `data_generation` an older page was asked for at, until it comes back (read or
    /// not). The timeline asks again after every rebuild; this keeps it to one request.
    older_asked: Option<u64>,
    /// Counts the times new timelines were read. A page of older saves asked for before one of
    /// them belongs to other data and is dropped.
    data_generation: u64,
    /// The last read of the timelines failed. The timeline keeps what it showed, and
    /// `shown_key` the key that belongs to, so the next refresh reads again.
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
    /// A Load (or "Carregar este save" on a bare save) is under way: another one waits.
    loading: bool,
    /// A Load just finished: the next timelines read scroll so that "Você está aqui" and
    /// "Agora" are both in view (spec §4.3 step 7).
    show_here: bool,
}

/// Whether a refresh that found the timelines key `key` has to read the timelines: the
/// timeline shows them as they were under another key (`shown_key`), or shows none yet.
fn needs_history(shown_key: &Option<String>, key: &str) -> bool {
    shown_key.as_deref() != Some(key)
}

/// What is selected in new `data`: the save `selected`, wherever it is now, while it is loaded;
/// otherwise "Now".
fn reselect(selected: Option<&SaveId>, data: &TimelineData) -> Selection {
    selected
        .and_then(|id| data.saves.iter().position(|save| save.id == *id))
        .map_or(Selection::Now, Selection::Station)
}

/// The counts in the header, "11 saves · 3 linhas": the saves (each counted once), and the
/// lines when there are several. The pending slot is not a lane, so it is not counted.
fn count_parts(saves: usize, lanes: usize) -> (i64, Option<i64>) {
    (count(saves), (lanes > 1).then(|| count(lanes)))
}

/// The line the peek banner names for the save at index `save`: its lane's name, unless that
/// lane also holds "Você está aqui" (the save at `here`).
fn peek_line(lanes: &[LaneData], here: Option<usize>, save: usize) -> Option<String> {
    let lane_of = |i: usize| lanes.iter().position(|lane| lane.saves.contains(&i));
    let lane = lane_of(save)?;
    (here.and_then(lane_of) != Some(lane)).then(|| lanes[lane].label.clone())
}

/// The names of lanes without one of their own, in the current language.
fn lane_names() -> LaneNames {
    LaneNames {
        main_line: tr("timeline.main_line"),
        no_line: tr("timeline.no_line"),
    }
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
            (ui.load_here, TabButton::LoadHere),
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
        if sender == live.peek.load {
            return Some(TabButton::Load);
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
        // The lanes without a name of their own are named in the language.
        self.rebuild(self.unsaved(), None);
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

    /// The app just made `save`: the next data that holds it grows it in. Reads the project
    /// again now.
    pub fn show_new_save(&self, save: SaveId) {
        if self.live.is_none() {
            return;
        }
        self.state.borrow_mut().grow_in(save);
        self.refresh();
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

    /// Removes the app's empty timelines (one left behind by a crash after Load, say) in the
    /// background, before the next refresh reads the project. Not while the project can only be
    /// looked at: a merge or a rebase under way is left alone. It is only housekeeping: a
    /// failure is logged, and the next sweep tries again.
    pub fn sweep(&self) {
        let Some(live) = &self.live else { return };
        live.worker.submit(|p| {
            match when_normal(p, sweep_empty_timelines) {
                Some(Ok(removed)) if !removed.is_empty() => {
                    log!("empty timelines removed: {}", removed.join(", "));
                }
                Some(Err(error)) => log!("empty timelines were not removed: {error}"),
                _ => {}
            }
            Box::new(|| {})
        });
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

    /// Reads the project's state again in the background, with every timeline when they look
    /// different from the ones shown (or reading them failed before), and shows it. Called by
    /// the folder watcher, when the window comes back to the front, by "Check again" and after
    /// every operation.
    pub fn refresh(&self) {
        let Some(live) = &self.live else { return };
        let (shown, loaded) = {
            let state = self.state.borrow();
            (state.shown_key.clone(), state.loaded())
        };
        let id = self.id;
        live.worker.submit_refresh(move |p| {
            let snap = snapshot(p);
            // As many saves as are loaded, so that the view keeps what it shows.
            let read = needs_history(&shown, &snap.timelines_key)
                .then(|| read_timelines(p, HISTORY_PAGE.max(loaded)));
            Box::new(move || with_tab(id, |tab| tab.apply_refresh(snap, read)))
        });
    }

    /// Shows a refresh's result. `read` is every timeline, read when the worker saw that the
    /// timelines key changed.
    fn apply_refresh(&self, snap: ProjectSnapshot, read: Option<Result<TimelinesView, CoreError>>) {
        let Some(live) = &self.live else { return };
        let unsaved = snap.changes.len();
        self.state.borrow_mut().head.clone_from(&snap.head);
        let stale = needs_history(&self.state.borrow().shown_key, &snap.timelines_key);
        match read {
            Some(read) if stale => self.show_timelines(live, &snap, read),
            _ => {
                if !stale {
                    self.state.borrow_mut().timelines_unchanged();
                    // A refresh that landed during the Load read the timelines already.
                    if std::mem::take(&mut self.state.borrow_mut().show_here) {
                        live.timeline.show_here_and_now();
                    }
                }
                live.timeline.set_unsaved(unsaved);
                // New data moves "Agora" to the current time by itself; this does it otherwise.
                live.timeline.tick_clock();
                if stale {
                    // The worker compared with a key the timeline no longer shows (another
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

    /// Gives the timeline the timelines just read, keeping its view and the selected save.
    /// When they could not be read, the timeline keeps what it shows (with "Now" up to date)
    /// and the next refresh reads again.
    fn show_timelines(
        &self,
        live: &Live,
        snap: &ProjectSnapshot,
        read: Result<TimelinesView, CoreError>,
    ) {
        let unsaved = snap.changes.len();
        let view = match read {
            Ok(view) => view,
            Err(error) => {
                // History objects missing (a synced `.git` folder) or git failing once: the
                // project's condition can still read Normal, so say so in the count instead.
                log!(
                    "the timelines of {} could not be read: {error}",
                    self.root.display()
                );
                self.state.borrow_mut().newest_page_failed();
                live.timeline.set_unsaved(unsaved);
                return;
            }
        };
        let (grow, show_here) = {
            let mut state = self.state.borrow_mut();
            state.newest_page_read(&snap.timelines_key, view);
            (state.grow.take(), std::mem::take(&mut state.show_here))
        };
        self.rebuild(unsaved, grow);
        if show_here {
            live.timeline.show_here_and_now();
        }
    }

    /// Lays the loaded timelines out in lanes and gives them to the timeline, with `unsaved`
    /// changes and the selected save still selected (or "Now" when it is gone, which leaves
    /// peek). With `grow`, that save grows in.
    fn rebuild(&self, unsaved: usize, grow: Option<SaveId>) {
        let Some(live) = &self.live else { return };
        let data = {
            let state = self.state.borrow();
            let Some(view) = &state.timelines else { return };
            let layout = assign_lanes(view);
            let mut data = timeline_data(view, &layout, &lane_names(), unsaved, None);
            data.selection = reselect(state.selected.as_ref(), &data);
            data
        };
        let selection = data.selection;
        let new_save = grow.and_then(|id| data.saves.iter().position(|save| save.id == id));
        match new_save {
            Some(i) => live.timeline.set_data_growing(data, i),
            None => live.timeline.set_data(data),
        }
        match selection {
            // The peeked save may sit on another lane than "Você está aqui" now.
            Selection::Station(i) => live.peek.set_line(self.line_of(i), live.lang.get()),
            Selection::Now if self.state.borrow().selected.is_some() => {
                self.on_select(Selection::Now);
            }
            Selection::Now => {}
        }
        self.show_count();
        self.show_load_button();
    }

    /// The unsaved changes as the last refresh counted them.
    fn unsaved(&self) -> usize {
        self.state
            .borrow()
            .shown
            .as_ref()
            .map_or(0, |(_, unsaved)| *unsaved)
    }

    /// The lanes the timeline shows, in display order.
    fn lanes(&self) -> Vec<LaneData> {
        let Some(live) = &self.live else {
            return Vec::new();
        };
        (0..).map_while(|lane| live.timeline.lane(lane)).collect()
    }

    /// The line the peek banner names for the save at index `i` of the timeline (see
    /// [`peek_line`]).
    fn line_of(&self, i: usize) -> Option<String> {
        let here = self.live.as_ref()?.timeline.here();
        peek_line(&self.lanes(), here, i)
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
            // Renaming and archiving come with the lane menu (Task 11).
            TimelineAction::LaneMenu { .. } | TimelineAction::Rename(_) => {}
        }
    }

    /// A station was selected on the timeline: a save opens peek mode, "Now" leaves it.
    fn on_select(&self, selection: Selection) {
        let selected = match selection {
            Selection::Station(i) => self
                .live
                .as_ref()
                .and_then(|live| live.timeline.save_at(i))
                .map(|save| save.id),
            Selection::Now => None,
        };
        self.state.borrow_mut().selected = selected;
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

    /// Peeks at the save at index `i` of the timeline's loaded saves, and reads what it changed
    /// and every file in it.
    fn enter_peek(&self, i: usize) {
        let Some(live) = &self.live else { return };
        let Some(save) = live.timeline.save_at(i) else {
            return;
        };
        if live.peek.peeked_id().as_ref() == Some(&save.id) {
            return;
        }
        let before_name = self.state.borrow().name_before(&save.id);
        let save_id = save.id.clone();
        live.peek
            .enter(save, before_name, self.line_of(i), live.lang.get());
        self.show_load_button();
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
        let (head, head_name) = {
            let state = self.state.borrow();
            let head = state.head.clone();
            let name = head.as_ref().and_then(|head| state.save_name(head));
            (head, name)
        };
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

    /// Shows "Carregar este save" for the save peeked at (spec §4.1): hidden on "Você está
    /// aqui", off with the reason in a project that can only be looked at, else with what
    /// Loading will do as its tooltip.
    fn show_load_button(&self) {
        let Some(live) = &self.live else { return };
        let Some(peeked) = live.peek.peeked_id() else {
            return;
        };
        let state = self.state.borrow();
        let button = load_button(&peeked, state.head.as_ref(), state.read_only);
        let tip = match button {
            LoadButton::Hidden => None,
            LoadButton::Disabled => self.restore_off(),
            LoadButton::Enabled => {
                let timelines: &[Timeline] = state
                    .timelines
                    .as_ref()
                    .map_or(&[], |view| view.timelines.as_slice());
                let names = lane_names();
                Some(load_tip(timelines, &peeked, |t| timeline_label(t, &names)))
            }
        };
        live.peek.show_load(button, tip.as_deref());
    }

    /// The save "Carregar este save" in the peek banner would load now: the one peeked at,
    /// while the button is on. `None` while another Load is under way.
    pub fn load_target(&self) -> Option<SaveId> {
        let live = self.live.as_ref()?;
        let peeked = live.peek.peeked_id()?;
        let state = self.state.borrow();
        let button = load_button(&peeked, state.head.as_ref(), state.read_only);
        (button == LoadButton::Enabled && !state.loading).then_some(peeked)
    }

    /// The name of the save `id`, when it is loaded on the timeline.
    pub fn save_name(&self, id: &SaveId) -> Option<String> {
        self.state.borrow().save_name(id)
    }

    /// Whether the folder has unsaved changes, as the last refresh found it.
    pub fn has_unsaved(&self) -> bool {
        self.unsaved() > 0
    }

    /// Marks a Load (or "Carregar este save" on a bare save) as under way, or over. Returns
    /// false, changing nothing, when one is under way already and `on`.
    pub fn set_loading(&self, on: bool) -> bool {
        let mut state = self.state.borrow_mut();
        if on && state.loading {
            return false;
        }
        state.loading = on;
        true
    }

    /// A Load moved the project: the selection goes back to "Agora", and once the timelines
    /// are read again the view shows both "Você está aqui" and "Agora" (spec §4.3 step 7).
    pub fn show_loaded(&self) {
        if self.live.is_none() {
            return;
        }
        self.back_to_now();
        self.state.borrow_mut().show_here = true;
        self.refresh();
    }

    /// Loads the page of saves before the oldest one loaded, across every timeline.
    fn load_older(&self) {
        let Some(live) = &self.live else { return };
        // Asked for already, and not back yet.
        let Some((loaded, generation)) = self.state.borrow_mut().ask_older() else {
            return;
        };
        let id = self.id;
        live.worker.submit(move |p| {
            let page = read_older_saves(p, loaded, HISTORY_PAGE);
            Box::new(move || with_tab(id, |tab| tab.apply_older(generation, page)))
        });
    }

    /// Adds an older page and lays the lanes out again; the view (and a temporary zoom) stays.
    /// A page asked for before the timeline got new data is dropped: it would add the wrong
    /// saves, and the timeline asks again by itself if it still needs older ones.
    fn apply_older(&self, generation: u64, page: Result<Vec<GraphSave>, CoreError>) {
        match page {
            Ok(page) => {
                if !self.state.borrow_mut().older_page_read(generation, page) {
                    return;
                }
                self.rebuild(self.unsaved(), None);
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

    /// "1500 saves · 3 linhas", "No saves yet", "More than 500 saves" while older ones are not
    /// loaded, or that the history could not be read. The lines are named when there are
    /// several.
    fn show_count(&self) {
        let Some(live) = &self.live else { return };
        let state = self.state.borrow();
        let n = live.timeline.save_count();
        let (saves, lines) = count_parts(n, self.lanes().len());
        let text = if state.read_failed() {
            tr("timeline.history_unreadable")
        } else if state.shown_key.is_none() {
            return;
        } else if n == 0 && !state.more_available() {
            tr("timeline.count_none")
        } else {
            let saves = if state.more_available() {
                tr_fmt("timeline.count_more", &[&n.to_string()])
            } else {
                tr_plural("timeline.count", saves)
            };
            match lines {
                Some(lines) => format!("{saves} · {}", tr_plural("timeline.lines", lines)),
                None => saves,
            }
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
        ui.load_here.set_visible(offers_load_here(condition));
        self.show_load_button();
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
            timelines: None,
            shown_key: None,
            selected: None,
            grow: None,
            older_asked: None,
            data_generation: 0,
            newest_failed: false,
            older_failed: false,
            head: None,
            read_only: false,
            shown: None,
            compare: None,
            compare_generation: 0,
            loading: false,
            show_here: false,
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

    /// The timelines were read (`view`) under the timelines key `key`, and the timeline gets
    /// them.
    fn newest_page_read(&mut self, key: &str, view: TimelinesView) {
        self.newest_failed = false;
        self.older_failed = false;
        self.data_generation += 1;
        self.timelines = Some(view);
        self.shown_key = Some(key.to_owned());
    }

    /// Reading the timelines failed. Nothing else changes: the timeline keeps what it shows,
    /// and since that is not under the new key the next refresh reads again.
    fn newest_page_failed(&mut self) {
        self.newest_failed = true;
    }

    /// An older page asked for at `generation` was read: its saves join the loaded ones.
    /// Returns false when the page belongs to data the timeline no longer has (then nothing
    /// changes).
    fn older_page_read(&mut self, generation: u64, page: Vec<GraphSave>) -> bool {
        self.older_back(generation);
        if generation != self.data_generation {
            return false;
        }
        let Some(view) = &mut self.timelines else {
            return false;
        };
        self.older_failed = false;
        let more_available = page.len() == HISTORY_PAGE;
        merge_older(view, page, more_available);
        true
    }

    /// Reading an older page asked for at `generation` failed: the older saves are still there,
    /// and the next refresh asks again. False when the page belongs to data the timeline no
    /// longer has (then nothing changes).
    fn older_page_failed(&mut self, generation: u64) -> bool {
        self.older_back(generation);
        if generation != self.data_generation {
            return false;
        }
        self.older_failed = true;
        true
    }

    /// An older page is to be asked for: where it starts (the saves loaded) and the generation
    /// it belongs to. `None` while one asked for at this generation is not back yet.
    fn ask_older(&mut self) -> Option<(usize, u64)> {
        if self.older_asked == Some(self.data_generation) {
            return None;
        }
        self.older_asked = Some(self.data_generation);
        Some((self.loaded(), self.data_generation))
    }

    /// The older page asked for at `generation` came back, read or not.
    fn older_back(&mut self, generation: u64) {
        if self.older_asked == Some(generation) {
            self.older_asked = None;
        }
    }

    /// The app just made `save`: the next data that holds it grows it in, unless the shown
    /// data holds it already (the refresh that read it came first).
    fn grow_in(&mut self, save: SaveId) {
        if self.find(&save).is_none() {
            self.grow = Some(save);
        }
    }

    /// A refresh found the timelines as shown: they hold the save just made, if any, so it
    /// must not grow in later on another read.
    fn timelines_unchanged(&mut self) {
        self.grow = None;
    }

    /// Whether an older page failed and is to be asked for again now (once).
    fn take_older_retry(&mut self) -> bool {
        std::mem::take(&mut self.older_failed)
    }

    /// Whether the last read of the history failed.
    fn read_failed(&self) -> bool {
        self.newest_failed || self.older_failed
    }

    /// Whether saves older than the loaded ones exist.
    fn more_available(&self) -> bool {
        self.timelines
            .as_ref()
            .is_some_and(|view| view.more_available)
    }

    /// How many saves are loaded, drawn or not: where the next older page starts.
    fn loaded(&self) -> usize {
        self.timelines.as_ref().map_or(0, |view| view.saves.len())
    }

    /// The loaded save `id`.
    fn find(&self, id: &SaveId) -> Option<&GraphSave> {
        let view = self.timelines.as_ref()?;
        view.saves.iter().find(|graph| graph.save.id == *id)
    }

    /// The name of the loaded save `id`.
    fn save_name(&self, id: &SaveId) -> Option<String> {
        self.find(id).map(|graph| graph.save.name.clone())
    }

    /// The name of the save just before `id` (its first parent), when both are loaded.
    fn name_before(&self, id: &SaveId) -> Option<String> {
        let parent = self.find(id)?.parents.first()?;
        self.save_name(parent)
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
        load_here: find("banner.load")?,
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
    use savestates_core::SaveState;

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

    /// Saves `n` in `range`, oldest first, one hour apart, each on top of the one before.
    fn graph_saves(range: std::ops::Range<usize>) -> Vec<GraphSave> {
        use chrono::{FixedOffset, TimeDelta, TimeZone};
        let base = FixedOffset::west_opt(3 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .unwrap();
        let id = |n: usize| SaveId(format!("{n:040x}"));
        range
            .map(|n| {
                let time = base + TimeDelta::hours(n as i64);
                GraphSave {
                    save: SaveState {
                        id: id(n),
                        name: format!("Save {n}"),
                        time,
                        committed: time,
                        files_changed: 1,
                    },
                    parents: n.checked_sub(1).map(id).into_iter().collect(),
                }
            })
            .collect()
    }

    /// The newest `len` saves of a one-timeline project of 1,000 saves, as `read_timelines`
    /// gives them.
    fn view(len: usize, more_available: bool) -> TimelinesView {
        let mut saves = graph_saves(1000 - len..1000);
        saves.reverse();
        TimelinesView {
            saves,
            more_available,
            ..TimelinesView::default()
        }
    }

    #[test]
    fn a_changed_timelines_key_reads_the_history_again() {
        let shown = Some("a".to_owned());
        assert!(needs_history(&shown, "b"));
        assert!(!needs_history(&shown, "a"));
        // Nothing shown yet: whatever the key, the history is read.
        assert!(needs_history(&None, "a"));
        assert!(needs_history(&None, ""));
    }

    #[test]
    fn the_selection_follows_its_save_across_reloads() {
        let saves = |range: std::ops::Range<usize>| -> Vec<SaveState> {
            graph_saves(range).into_iter().map(|g| g.save).collect()
        };
        let data = TimelineData::one_lane(saves(10..20), true, 0);
        let picked = SaveId(format!("{:040x}", 12));
        assert_eq!(reselect(Some(&picked), &data), Selection::Station(2));
        // An older page in front of it: the same save, further along.
        let data = TimelineData::one_lane(saves(0..20), false, 0);
        assert_eq!(reselect(Some(&picked), &data), Selection::Station(12));
        // The save is gone (another program rewrote the history): back to Now.
        let data = TimelineData::one_lane(saves(13..20), false, 0);
        assert_eq!(reselect(Some(&picked), &data), Selection::Now);
        // Nothing was selected.
        assert_eq!(reselect(None, &data), Selection::Now);
    }

    #[test]
    fn a_failed_history_read_is_read_again_by_the_next_refresh() {
        let mut state = TabState::new();
        assert!(needs_history(&state.shown_key, "a"));

        // The first read fails: nothing is recorded as shown, so the next refresh reads again.
        state.newest_page_failed();
        assert!(state.read_failed());
        assert!(needs_history(&state.shown_key, "a"));
        assert_eq!(state.data_generation, 0);

        // It works: those timelines are shown, and refreshes that find the same key read nothing.
        state.newest_page_read("a", view(HISTORY_PAGE, true));
        assert!(!state.read_failed());
        assert!(state.more_available());
        assert_eq!(state.loaded(), HISTORY_PAGE);
        assert_eq!(state.data_generation, 1);
        assert!(!needs_history(&state.shown_key, "a"));

        // New timelines that cannot be read keep the old ones on screen, and their key, so the
        // read is tried again.
        state.newest_page_failed();
        assert!(needs_history(&state.shown_key, "b"));
        assert!(state.more_available());
        assert_eq!(state.loaded(), HISTORY_PAGE);
        assert_eq!(state.data_generation, 1);
        state.newest_page_read("b", view(3, false));
        assert!(!state.more_available());
        assert_eq!(state.loaded(), 3);
        assert!(!needs_history(&state.shown_key, "b"));

        // A project without saves shows that, and is not read again.
        let mut empty = TabState::new();
        empty.newest_page_read("", TimelinesView::default());
        assert!(!needs_history(&empty.shown_key, ""));
        assert_eq!(empty.loaded(), 0);
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
        state.newest_page_read("a", view(HISTORY_PAGE, true));
        let generation = state.data_generation;

        // The read fails: more saves are still there to load, and the next refresh asks again.
        assert!(state.older_page_failed(generation));
        assert!(state.more_available());
        assert!(state.read_failed());
        assert!(state.take_older_retry());
        assert!(!state.take_older_retry(), "asked again once");

        // The retry works: 380 older saves, and no more after them. A save the page repeats
        // (pages read along several timelines overlap) is loaded once.
        let mut older = graph_saves(1000 - HISTORY_PAGE - 380..1000 - HISTORY_PAGE + 1);
        older.reverse();
        assert!(state.older_page_read(generation, older));
        assert!(!state.more_available());
        assert_eq!(state.loaded(), HISTORY_PAGE + 380);
        assert!(!state.read_failed());

        // A page asked for before the timeline got new data is dropped, failed or not, and new
        // timelines forget a pending retry.
        assert!(state.older_page_failed(generation));
        state.newest_page_read("b", view(HISTORY_PAGE, true));
        assert!(!state.take_older_retry());
        assert!(!state.older_page_read(generation, graph_saves(0..HISTORY_PAGE)));
        assert_eq!(state.loaded(), HISTORY_PAGE);
        assert!(state.more_available());
        assert!(!state.older_page_failed(generation));
        assert!(!state.take_older_retry());
    }

    #[test]
    fn an_older_page_is_asked_for_once_until_it_comes_back() {
        let mut state = TabState::new();
        state.newest_page_read("a", view(HISTORY_PAGE, true));
        let generation = state.data_generation;
        // The timeline asks again after every rebuild (a new language, say): one request goes
        // out, for the saves after the loaded ones.
        assert_eq!(state.ask_older(), Some((HISTORY_PAGE, generation)));
        assert_eq!(state.ask_older(), None);
        // It comes back: the next one may go out, from further back.
        let mut older = graph_saves(1000 - 2 * HISTORY_PAGE..1000 - HISTORY_PAGE);
        older.reverse();
        assert!(state.older_page_read(generation, older));
        assert_eq!(state.ask_older(), Some((2 * HISTORY_PAGE, generation)));
        // It fails: the retry may go out.
        assert!(state.older_page_failed(generation));
        assert_eq!(state.ask_older(), Some((2 * HISTORY_PAGE, generation)));
        // New timelines: a request for them goes out, whatever was asked for before.
        state.newest_page_read("b", view(HISTORY_PAGE, true));
        assert_eq!(state.ask_older(), Some((HISTORY_PAGE, generation + 1)));
        // The page asked for before them comes back late: it frees nothing.
        assert!(!state.older_page_read(generation, Vec::new()));
        assert_eq!(state.ask_older(), None);
    }

    #[test]
    fn a_save_grows_in_once() {
        let mut state = TabState::new();
        state.newest_page_read("a", view(10, false));
        // A save already shown (the refresh that read it came first) does not grow again.
        state.grow_in(SaveId(format!("{:040x}", 995)));
        assert_eq!(state.grow, None);
        let new = SaveId(format!("{:040x}", 1000));
        state.grow_in(new.clone());
        assert_eq!(state.grow, Some(new));
        // A refresh that finds the timelines as shown forgets it: they already hold it.
        state.timelines_unchanged();
        assert_eq!(state.grow, None);
    }

    #[test]
    fn the_count_names_lines_only_when_there_are_several() {
        assert_eq!(count_parts(11, 3), (11, Some(3)));
        assert_eq!(count_parts(1500, 2), (1500, Some(2)));
        assert_eq!(count_parts(5, 1), (5, None));
        assert_eq!(count_parts(0, 0), (0, None));
    }

    #[test]
    fn the_peek_banner_names_the_line_unless_it_holds_you_are_here() {
        let lane = |label: &str, saves: std::ops::Range<usize>| LaneData {
            label: label.to_owned(),
            branch: Some(label.to_owned()),
            color: crate::theme::LINE,
            saves,
            fork: None,
            continues_left: false,
            archived: false,
        };
        let lanes = [lane("Linha principal", 0..5), lane("Menu lateral", 5..9)];
        // "Você está aqui" on the main line: a save there is not named, one on the other is.
        assert_eq!(peek_line(&lanes, Some(4), 2), None);
        assert_eq!(
            peek_line(&lanes, Some(4), 6),
            Some("Menu lateral".to_owned())
        );
        // On the other line, it is the other way round.
        assert_eq!(peek_line(&lanes, Some(8), 6), None);
        assert_eq!(
            peek_line(&lanes, Some(8), 2),
            Some("Linha principal".to_owned())
        );
    }

    #[test]
    fn the_save_before_is_its_first_parent() {
        let mut state = TabState::new();
        // A side line s10 <- s11 off s3, next to the main line s0..s9.
        let mut saves = graph_saves(0..10);
        let mut side = graph_saves(10..12);
        side[0].parents = vec![SaveId(format!("{:040x}", 3))];
        saves.extend(side);
        saves.reverse();
        let v = TimelinesView {
            saves,
            ..TimelinesView::default()
        };
        state.newest_page_read("a", v);
        let id = |n: usize| SaveId(format!("{n:040x}"));
        assert_eq!(state.name_before(&id(11)).as_deref(), Some("Save 10"));
        assert_eq!(state.name_before(&id(10)).as_deref(), Some("Save 3"));
        assert_eq!(state.name_before(&id(5)).as_deref(), Some("Save 4"));
        assert_eq!(state.name_before(&id(0)), None);
        assert_eq!(state.save_name(&id(9)).as_deref(), Some("Save 9"));
        assert_eq!(state.save_name(&id(99)), None);
    }
}
