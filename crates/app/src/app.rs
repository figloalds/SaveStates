//! The app: startup, the main window, opening projects into tabs, and what is remembered between
//! runs.
//!
//! Everything here runs on Raven's UI thread. Work that blocks (git) runs on other threads and
//! comes back through `task::post`; a posted closure cannot hold the app (it is not `Send`), so it
//! finds it again with [`with_app`] and a tab with [`with_tab`].

use std::cell::{Cell, OnceCell, RefCell};
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use savestates_ai::{CancelToken, NamingError, NamingRequest, generate_name, locate_claude};
use savestates_core::settings::{self, NamingMode, Settings, WindowGeom};
use savestates_core::{
    CoreError, OpenOutcome, Project, RepoCondition, SaveError, SaveId, SaveState, apply_decisions,
    commit_all, identity_missing, naming_input, open_project, scan, set_identity_if_missing,
    snapshot, start_saving_folder, timeline_page,
};
use savestates_git::{GitError, GitExe};
use savestates_i18n::{Lang, TRANSLATIONS_CSV, resolve, startup_failure_text, system_language};
use savestates_ui_raven::{
    self as ui, Backend, Key, Rgba, Widget, Window, bind_action, dialog, locale, tabs as tab_view,
    task, tr,
};

use crate::dialogs::{ai_spinner, confirm, identity, inform, name, warnings};
use crate::flows::quick_save::{Effect, QsContext, QsEvent, QuickSave};
use crate::start_screen::{GIT_DOWNLOAD_URL, StartScreen};
use crate::tabs::{ProjectTab, RestoreState, TabButton, TabId, TabOp, TabsState, same_folder};
use crate::{theme, toast, tr_key, ui_files};

/// How long a toast that only informs stays up.
const NOTICE_SECONDS: f64 = 8.0;
/// A window at or beyond these coordinates is minimized (Windows parks it at -32000).
const OFF_SCREEN: i32 = -10_000;
/// While the last run's tabs reopen, the start screen shows if none is back after this long.
const RESTORE_GRACE_SECONDS: f64 = 0.5;

/// How a folder came to be opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Opening {
    /// The user asked for it: it goes first in the recent projects and its tab becomes active.
    ByUser,
    /// A tab of the last run coming back. Only the one that was active becomes active again.
    Restored { was_active: bool },
}

impl Opening {
    fn activates(self) -> bool {
        match self {
            Opening::ByUser => true,
            Opening::Restored { was_active } => was_active,
        }
    }
}

thread_local! {
    /// The app, once started. Only the UI thread's is ever set.
    static APP: OnceCell<Rc<App>> = const { OnceCell::new() };
}

/// Runs `f` with the app, if it is running (on the UI thread).
pub fn with_app(f: impl FnOnce(&Rc<App>)) {
    if let Some(app) = APP.with(|app| app.get().cloned()) {
        f(&app);
    }
}

/// Runs `f` with the tab `id`, if it is still open.
pub fn with_tab(id: TabId, f: impl FnOnce(&ProjectTab)) {
    let mut tab = None;
    with_app(|app| tab = app.tab(id));
    if let Some(tab) = tab {
        f(&tab);
    }
}

pub struct App {
    window: Window,
    settings_path: PathBuf,
    settings: RefCell<Settings>,
    lang: Lang,
    /// `None` when Git is missing: the app then shows only the screen that says so.
    git: Option<GitExe>,
    tab_view: Widget,
    start_host: Widget,
    open_button: Widget,
    start: RefCell<StartScreen>,
    tabs: RefCell<TabsState>,
    /// The last run's tabs while they reopen.
    restore: RefCell<RestoreState>,
    /// The open tabs, in the order of `tabs`.
    pages: RefCell<Vec<Rc<ProjectTab>>>,
    next_id: Cell<TabId>,
    /// The Quick Save under way, if any.
    quick_save: RefCell<QuickSaveRun>,
}

/// The Quick Save flow (at most one at a time in the app), and what the app keeps for the run
/// under way.
struct QuickSaveRun {
    flow: QuickSave,
    /// Counts the runs: what comes back for an earlier one (from the project's thread, the AI or
    /// a dialog) is dropped.
    run: u64,
    /// The tab whose project it saves.
    tab: TabId,
    /// What runs once the save is made ("Quick Save first"), and only then.
    then: Option<Box<dyn FnOnce()>>,
    /// The Claude CLI, as the scan found it (looked for in AI naming only).
    claude: Option<PathBuf>,
    /// Stops the AI while it thinks.
    cancel: Option<CancelToken>,
    /// The save just made and the one before it, read back for the timeline.
    made: Option<(SaveState, Option<SaveId>)>,
}

impl QuickSaveRun {
    /// No Quick Save under way, the last one being `run`.
    fn idle(run: u64) -> QuickSaveRun {
        QuickSaveRun {
            flow: QuickSave::new(),
            run,
            tab: 0,
            then: None,
            claude: None,
            cancel: None,
            made: None,
        }
    }
}

/// What the scan before a Quick Save found.
enum Scanned {
    /// The project can only be looked at now (another program started a merge since the last
    /// refresh, say): nothing is saved.
    ReadOnly,
    /// What to tell the flow: the scan, or that there is nothing to save, or that it failed.
    Done(QsEvent),
}

impl App {
    /// Starts the app and runs it until its window closes. Returns the process exit code.
    pub fn run() -> i32 {
        let settings_path = settings::default_path();
        let settings = settings::load(&settings_path);
        let lang = resolve(settings.language, &system_language());
        if ui::init(Backend::Vulkan).is_err() {
            let (title, body) = startup_failure_text(lang);
            ui::native_error_box(title, body);
            return 1;
        }
        load_theme();
        load_translations(lang);
        let git = savestates_git::locate().ok();
        let app = match App::new(settings_path, settings, lang, git) {
            Ok(app) => app,
            Err(error) => {
                eprintln!("the main window could not be built: {error}");
                ui::shutdown();
                return 1;
            }
        };
        APP.with(|cell| {
            cell.get_or_init(|| app.clone());
        });
        app.wire();
        app.restore_window();
        if app.git.is_some() {
            app.restore_tabs();
        } else {
            app.show_git_missing();
        }
        while ui::wait_frame(-1.0) {}
        ui::shutdown();
        0
    }

    fn new(
        settings_path: PathBuf,
        settings: Settings,
        lang: Lang,
        git: Option<GitExe>,
    ) -> Result<Rc<App>, String> {
        let window = Window::from_xml(&ui_files::source("main.xml"))?;
        let find = |name: &str| {
            window
                .find(name)
                .ok_or_else(|| format!("main.xml has no widget named {name}"))
        };
        let start = StartScreen::new()?;
        let start_host = find("start_host")?;
        start_host.add_child(start.widget());
        toast::install(window);
        Ok(Rc::new(App {
            window,
            settings_path,
            settings: RefCell::new(settings),
            lang,
            git,
            tab_view: find("tabs")?,
            start_host,
            open_button: find("open_project")?,
            start: RefCell::new(start),
            tabs: RefCell::default(),
            restore: RefCell::default(),
            pages: RefCell::default(),
            next_id: Cell::new(1),
            quick_save: RefCell::new(QuickSaveRun::idle(0)),
        }))
    }

    /// Connects the window's buttons and events.
    fn wire(&self) {
        self.window.on_close(|| {
            with_app(|app| app.save_settings());
            true
        });
        self.window
            .on_focus_gained(|| with_app(|app| app.refresh_active()));
        bind_action("app.open_project", |_| with_app(|app| app.ask_for_folder()));
        bind_action("start.open_recent", |sender| {
            with_app(|app| {
                let folder = app.start.borrow().recent_for(sender);
                if let Some(folder) = folder {
                    app.open_folder(folder);
                }
            });
        });
        bind_action("start.download_git", |_| {
            ui::open_url(GIT_DOWNLOAD_URL);
        });
        bind_action("app.quick_save", |_| {
            with_app(|app| app.quick_save_active())
        });
        self.window
            .register_shortcut(Key::Char('S'), true, false, "app.quick_save");
        for action in [
            "tab.jump_to_now",
            "tab.quick_save",
            "tab.check_again",
            "tab.remove",
            "tab.back_to_now",
            "tab.bring_back_everything",
            "tab.bring_back",
        ] {
            bind_action(action, |sender| with_app(|app| app.tab_button(sender)));
        }
        tab_view::on_changed(self.tab_view, |i| {
            with_app(|app| app.tabs.borrow_mut().activate(i));
        });
        tab_view::on_close(self.tab_view, |page| {
            with_app(|app| app.tab_closed(page));
            true
        });
    }

    /// Puts the window back where it was, then shows it.
    fn restore_window(&self) {
        let geometry = self.settings.borrow().window;
        if let Some(g) = geometry {
            if g.width > 0 && g.height > 0 {
                self.window.set_size(g.width, g.height);
            }
            if g.x > OFF_SCREEN && g.y > OFF_SCREEN {
                self.window.set_position(g.x, g.y);
            }
        }
        self.window.show();
        if geometry.is_some_and(|g| g.maximized) {
            self.window.maximize();
        }
    }

    /// Reopens the tabs of the last run in the background, one by one in their order, each as
    /// soon as it is open; the one that was active becomes active again when it is back.
    fn restore_tabs(&self) {
        let (folders, active_tab) = {
            let settings = self.settings.borrow();
            (settings.open_tabs.clone(), settings.active_tab)
        };
        let Some(git) = self.git.clone().filter(|_| !folders.is_empty()) else {
            self.show_pages();
            return;
        };
        *self.restore.borrow_mut() = RestoreState::new(folders.clone(), active_tab);
        // The (empty) tab view shows while the first tab opens, rather than the start screen
        // flashing by; the start screen comes if none is back after a moment.
        self.start_host.set_visible(false);
        self.tab_view.set_visible(true);
        let _ = task::timer(RESTORE_GRACE_SECONDS, false, || {
            with_app(|app| app.show_pages());
        });
        let started = run_in_background(move || {
            for folder in folders {
                let outcome = open_project(&git, &folder);
                task::post(move || with_app(|app| app.restored(folder, outcome)));
            }
        });
        if !started {
            // The tabs stay saved for next time (the restore never ends, so nothing replaces
            // them); meanwhile the start screen offers to open a project.
            self.show_pages();
        }
    }

    /// A tab of the last run is back (or failed to open).
    fn restored(&self, folder: PathBuf, outcome: Result<OpenOutcome, CoreError>) {
        let was_active = self.restore.borrow_mut().arrived(&folder);
        self.opened(folder, outcome, Opening::Restored { was_active });
        self.show_pages();
    }

    fn show_git_missing(&self) {
        self.open_button.set_visible(false);
        self.start.borrow().show_git_missing();
        self.tab_view.set_visible(false);
        self.start_host.set_visible(true);
    }

    /// Shows the tabs, or the start screen with the recent projects when none is open.
    fn show_pages(&self) {
        let empty = self.pages.borrow().is_empty();
        if empty {
            let recent = self.settings.borrow().recent.clone();
            self.start.borrow_mut().set_recent(&recent);
        }
        self.start_host.set_visible(empty);
        self.tab_view.set_visible(!empty);
    }

    fn ask_for_folder(&self) {
        if self.git.is_none() {
            return;
        }
        if let Some(folder) = dialog::select_folder(&tr("project.open")) {
            self.open_folder(folder);
        }
    }

    /// Opens `folder` in the background, then as the active tab, and puts it first in the recent
    /// projects.
    fn open_folder(&self, folder: PathBuf) {
        let Some(git) = self.git.clone() else { return };
        run_in_background(move || {
            let outcome = open_project(&git, &folder);
            task::post(move || with_app(|app| app.opened(folder, outcome, Opening::ByUser)));
        });
    }

    fn opened(&self, folder: PathBuf, outcome: Result<OpenOutcome, CoreError>, how: Opening) {
        match outcome {
            Ok(OpenOutcome::Opened(project)) => {
                if how == Opening::ByUser {
                    self.remember(project.root());
                }
                self.add_project(project, how.activates());
            }
            Ok(OpenOutcome::NotARepo) => self.ask_start_saving(folder),
            Ok(OpenOutcome::Missing) => self.add_missing(folder, how.activates()),
            Err(error) => {
                eprintln!("{} could not be opened: {error}", folder.display());
                toast::show(&tr("project.open_failed"), None, NOTICE_SECONDS);
            }
        }
    }

    /// Asks whether to start saving a folder that is not a project yet.
    fn ask_start_saving(&self, folder: PathBuf) {
        let body = format!("{}\n\n{}", tr("project.ask_start_saving"), folder.display());
        confirm(
            self.window,
            tr_key("project.start_saving"),
            &body,
            &[(tr_key("common.yes"), 1), (tr_key("common.no"), 0)],
            move |answer| {
                if answer == 1 {
                    with_app(|app| app.start_saving(folder));
                }
            },
        );
    }

    fn start_saving(&self, folder: PathBuf) {
        let Some(git) = self.git.clone() else { return };
        run_in_background(move || {
            let result = start_saving_folder(&git, &folder);
            task::post(move || {
                with_app(|app| match result {
                    Ok(project) => {
                        app.remember(project.root());
                        if let Some(tab) = app.add_project(project, true) {
                            app.offer_first_save(&tab);
                        }
                    }
                    Err(error) => {
                        eprintln!("{} could not start saving: {error}", folder.display());
                        toast::show(&tr("project.start_saving_failed"), None, NOTICE_SECONDS);
                    }
                });
            });
        });
    }

    /// Offers the first save of a folder that just started saving: a Quick Save whose name is
    /// prefilled with "Começo do projeto" / "Project start", which the user can edit.
    fn offer_first_save(&self, tab: &ProjectTab) {
        self.start_quick_save(tab.id, Some(tr("save.first_name")));
    }

    /// Shows `project` in a new tab, or finds the tab that already shows it; either becomes the
    /// active tab when `activate`.
    fn add_project(&self, project: Project, activate: bool) -> Option<Rc<ProjectTab>> {
        let root = project.root().to_path_buf();
        match self.open_in_tabs(root.clone(), activate) {
            TabOp::Activated(i) => {
                let tab = self.pages.borrow().get(i).cloned()?;
                if tab.is_missing() {
                    // The folder is back (a drive plugged in again): a live tab replaces it.
                    self.forget_tab(i);
                    tab.page.destroy();
                    return self.add_project(project, activate);
                }
                self.show_active_tab();
                tab.refresh();
                Some(tab)
            }
            TabOp::Added(i) => {
                let zoom = self
                    .settings
                    .borrow()
                    .timeline_zoom
                    .get(&zoom_key(&root))
                    .copied();
                match ProjectTab::open(self.new_id(), project, self.lang, zoom) {
                    Ok(tab) => {
                        self.insert_tab(tab.clone());
                        tab.prune_recovery();
                        tab.refresh();
                        Some(tab)
                    }
                    Err(error) => {
                        self.tabs.borrow_mut().close(i);
                        eprintln!("the tab could not be built: {error}");
                        toast::show(&tr("project.open_failed"), None, NOTICE_SECONDS);
                        None
                    }
                }
            }
        }
    }

    /// Shows a folder that does not exist in its own tab, which offers to forget it.
    fn add_missing(&self, folder: PathBuf, activate: bool) {
        match self.open_in_tabs(folder.clone(), activate) {
            TabOp::Activated(_) => self.show_active_tab(),
            TabOp::Added(i) => match ProjectTab::missing(self.new_id(), folder) {
                Ok(tab) => self.insert_tab(tab),
                Err(error) => {
                    self.tabs.borrow_mut().close(i);
                    eprintln!("the tab could not be built: {error}");
                }
            },
        }
    }

    /// Opens `root` in `tabs`, keeping the active tab as it was unless `activate`.
    fn open_in_tabs(&self, root: PathBuf, activate: bool) -> TabOp {
        let mut tabs = self.tabs.borrow_mut();
        let before = tabs.active();
        let op = tabs.open(root);
        if let (false, Some(before)) = (activate, before) {
            tabs.activate(before);
        }
        op
    }

    /// Puts a tab that `tabs` just added (as its last one) into the tab view.
    fn insert_tab(&self, tab: Rc<ProjectTab>) {
        // The tab view may switch to a tab it gets and report it: `tabs` decides instead.
        let active = self.tabs.borrow().active();
        tab_view::add(self.tab_view, &tab.title(), tab.page);
        tab_view::set_closable(self.tab_view, tab.page, true);
        if let Some(active) = active {
            self.tabs.borrow_mut().activate(active);
        }
        self.pages.borrow_mut().push(tab);
        self.show_active_tab();
        self.show_pages();
        self.save_settings();
    }

    /// Makes the tab view show the tab that `tabs` has active.
    fn show_active_tab(&self) {
        let active = self.tabs.borrow().active();
        if let Some(active) = active {
            tab_view::set_active(self.tab_view, active);
        }
    }

    /// The user closed the tab whose page is `page`; the tab view deletes the page.
    fn tab_closed(&self, page: Widget) {
        let i = self.pages.borrow().iter().position(|tab| tab.page == page);
        if let Some(i) = i {
            self.forget_tab(i);
        }
    }

    /// Takes the tab at `i` out of the app (its page stays to the caller), remembering its zoom.
    fn forget_tab(&self, i: usize) {
        let tab = self.pages.borrow_mut().remove(i);
        let saving = {
            let quick_save = self.quick_save.borrow();
            quick_save.flow.is_busy() && quick_save.tab == tab.id
        };
        if saving {
            self.abandon_quick_save();
        }
        if let Some(zoom) = tab.zoom() {
            self.settings
                .borrow_mut()
                .timeline_zoom
                .insert(zoom_key(&tab.root), zoom);
        }
        self.tabs.borrow_mut().close(i);
        self.save_settings();
        // The tab view finishes removing the page after this returns: line it up afterwards.
        task::post(|| {
            with_app(|app| {
                app.show_active_tab();
                app.show_pages();
            });
        });
    }

    /// "Remove from list" in the tab of a folder that is gone: closes it and forgets the folder.
    fn remove_missing(&self, i: usize) {
        let Some(tab) = self.pages.borrow().get(i).cloned() else {
            return;
        };
        self.settings
            .borrow_mut()
            .recent
            .retain(|folder| !same_folder(folder, &tab.root));
        self.forget_tab(i);
        tab.page.destroy();
    }

    fn tab_button(&self, sender: Widget) {
        let found = self
            .pages
            .borrow()
            .iter()
            .enumerate()
            .find_map(|(i, tab)| tab.button(sender).map(|button| (i, tab.clone(), button)));
        let Some((i, tab, button)) = found else {
            return;
        };
        match button {
            TabButton::JumpToNow => tab.jump_to_now(),
            TabButton::QuickSave => self.quick_save_tab(&tab),
            TabButton::CheckAgain => tab.refresh(),
            TabButton::Remove => self.remove_missing(i),
            TabButton::BackToNow => tab.back_to_now(),
            TabButton::BringBackEverything => tab.bring_back_everything(self.window),
            TabButton::BringBack(which) => tab.bring_back(self.window, Some(which)),
            TabButton::BringBackThisFile => tab.bring_back(self.window, None),
        }
    }

    /// The main window, for the dialogs shown in it.
    pub fn window(&self) -> Window {
        self.window
    }

    /// Quick Saves the project of the tab `tab`, then runs `continuation` once the save is made
    /// ("Quick Save first", before bringing everything back over unsaved changes). When the
    /// Quick Save ends any other way (cancelled, failed, nothing to save), or another one is
    /// under way, `continuation` is dropped without running.
    pub fn quick_save_then(&self, tab: TabId, continuation: Box<dyn FnOnce()>) {
        self.begin_quick_save(tab, None, Some(continuation));
    }

    fn refresh_active(&self) {
        let active = self.tabs.borrow().active();
        let tab = active.and_then(|i| self.pages.borrow().get(i).cloned());
        if let Some(tab) = tab {
            tab.refresh();
        }
    }

    fn tab(&self, id: TabId) -> Option<Rc<ProjectTab>> {
        self.pages.borrow().iter().find(|tab| tab.id == id).cloned()
    }

    fn new_id(&self) -> TabId {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }

    /// Puts `root` first in the recent projects.
    fn remember(&self, root: &Path) {
        self.settings.borrow_mut().push_recent(root.to_path_buf());
    }

    /// Writes the settings: the window's place, the open tabs, the active one and each tab's
    /// timeline zoom. Only ever called on the UI thread, one write at a time.
    fn save_settings(&self) {
        let mut settings = self.settings.borrow_mut();
        // Without Git no tab could open: keep the ones of the last run for when it is back.
        // While they reopen, the ones still on their way are kept too.
        if self.git.is_some() {
            let tabs = self.tabs.borrow();
            let (open_tabs, active_tab) = self
                .restore
                .borrow()
                .tabs_to_save(tabs.roots(), tabs.active());
            settings.open_tabs = open_tabs;
            settings.active_tab = active_tab;
            for tab in self.pages.borrow().iter() {
                if let Some(zoom) = tab.zoom() {
                    settings.timeline_zoom.insert(zoom_key(&tab.root), zoom);
                }
            }
        }
        settings.window = self.geometry(settings.window);
        if let Err(error) = settings::save(&self.settings_path, &settings) {
            eprintln!("the settings could not be saved: {error}");
        }
    }

    /// Where the window is now. A maximized window keeps the place it goes back to, and a
    /// minimized one (or one not shown yet) changes nothing.
    fn geometry(&self, before: Option<WindowGeom>) -> Option<WindowGeom> {
        let (x, y) = self.window.position();
        let (width, height) = self.window.size();
        if self.window.is_maximized() {
            let normal = before.unwrap_or(WindowGeom {
                x,
                y,
                width,
                height,
                maximized: true,
            });
            return Some(WindowGeom {
                maximized: true,
                ..normal
            });
        }
        if width <= 0 || height <= 0 || x <= OFF_SCREEN || y <= OFF_SCREEN {
            return before;
        }
        Some(WindowGeom {
            x,
            y,
            width,
            height,
            maximized: false,
        })
    }
}

/// Quick Save (spec §6.7): [`QuickSave`] decides, and this carries out its effects. Git work
/// (the scan, the decisions, who is saving, reading what changed for the AI, the save) runs on
/// the tab's worker; the AI runs on a thread of its own. Every outcome (from those threads,
/// through `task::post`, or from a dialog) comes back tagged with its run, and is dropped if
/// another run started since.
impl App {
    /// Ctrl+S: a Quick Save of the active tab.
    fn quick_save_active(&self) {
        let active = self.tabs.borrow().active();
        let tab = active.and_then(|i| self.pages.borrow().get(i).cloned());
        if let Some(tab) = tab {
            self.quick_save_tab(&tab);
        }
    }

    /// "Quick Save" or Ctrl+S on `tab`: nothing in a project that can only be looked at. (With
    /// nothing to save, the scan says so.)
    fn quick_save_tab(&self, tab: &ProjectTab) {
        if tab.can_save() {
            self.start_quick_save(tab.id, None);
        }
    }

    /// Starts a Quick Save of the tab `tab`; with `prefill`, the name dialog starts with it
    /// instead of asking the AI. Nothing happens while another Quick Save is under way.
    pub fn start_quick_save(&self, tab: TabId, prefill: Option<String>) {
        self.begin_quick_save(tab, prefill, None);
    }

    fn begin_quick_save(
        &self,
        tab: TabId,
        prefill: Option<String>,
        then: Option<Box<dyn FnOnce()>>,
    ) {
        let run = {
            let mut quick_save = self.quick_save.borrow_mut();
            if quick_save.flow.is_busy() {
                return;
            }
            let run = quick_save.run + 1;
            *quick_save = QuickSaveRun {
                tab,
                then,
                ..QuickSaveRun::idle(run)
            };
            run
        };
        self.quick_save_event(run, QsEvent::Start { prefill });
    }

    /// Gives `ev` to the flow, if `run` is still the run under way, and carries out what it
    /// says. When the save was made, what was to run after it runs.
    fn quick_save_event(&self, run: u64, ev: QsEvent) {
        let naming = self.settings.borrow().naming;
        let (effects, done) = {
            let mut quick_save = self.quick_save.borrow_mut();
            if quick_save.run != run {
                return;
            }
            let ctx = QsContext {
                naming,
                claude_available: quick_save.claude.is_some(),
            };
            let effects = quick_save.flow.step(ev, &ctx);
            (effects, !quick_save.flow.is_busy())
        };
        let saved = effects
            .iter()
            .any(|effect| matches!(effect, Effect::AnimateNewStation(_)));
        for effect in effects {
            self.carry_out(run, effect);
        }
        if !done {
            return;
        }
        let then = {
            let mut quick_save = self.quick_save.borrow_mut();
            if quick_save.run != run {
                return;
            }
            quick_save.cancel = None;
            quick_save.then.take()
        };
        if let Some(then) = then.filter(|_| saved) {
            then();
        }
    }

    fn carry_out(&self, run: u64, effect: Effect) {
        let tab = self.tab(self.quick_save.borrow().tab);
        let Some(tab) = tab else {
            self.abandon_quick_save();
            return;
        };
        let window = self.window;
        match effect {
            Effect::RunScan => self.run_scan(run, &tab),
            Effect::ShowWarnings(flags) => warnings::show(window, flags, move |answer| {
                let ev = answer.map_or(QsEvent::WarningsCancelled, QsEvent::WarningsAnswered);
                with_app(|app| app.quick_save_event(run, ev));
            }),
            Effect::ApplyDecisions(decisions) => tab.submit(move |p| {
                let ev = match apply_decisions(p, &decisions) {
                    Ok(decision) => QsEvent::DecisionsApplied(decision),
                    Err(error) => QsEvent::SaveDone(Err(save_error(error))),
                };
                back_to(run, ev)
            }),
            Effect::AskIdentity => identity::show(window, move |answer| {
                with_app(|app| app.identity_answered(run, answer));
            }),
            Effect::ShowAiSpinner => ai_spinner::show(window, move || {
                with_app(|app| app.quick_save_event(run, QsEvent::AiCancelRequested));
            }),
            Effect::HideAiSpinner => ai_spinner::hide(),
            Effect::RunAi { excluded } => self.run_ai(run, &tab, excluded),
            Effect::CancelAi => {
                if let Some(cancel) = self.quick_save.borrow_mut().cancel.take() {
                    cancel.cancel();
                }
            }
            Effect::ShowNameDialog { prefill, note } => {
                name::show(window, &prefill, note, move |answer| {
                    let ev = answer.map_or(QsEvent::NameCancelled, QsEvent::NameConfirmed);
                    with_app(|app| app.quick_save_event(run, ev));
                });
            }
            Effect::RunCommit { name, checked } => self.run_commit(run, &tab, name, checked),
            Effect::SelectFile(path) => tab.select_file(&path),
            Effect::ShowHookRejected(output) => inform(
                window,
                tr_key("save.quick"),
                &tr("save.hook_rejected"),
                Some(&output),
            ),
            Effect::ShowChangedDuringSave(paths) => {
                let body = format!("{}\n\n{}", tr("save.changed_during_save"), paths.join("\n"));
                inform(window, tr_key("save.quick"), &body, None);
            }
            Effect::ShowLocked => inform(window, tr_key("save.quick"), &tr("error.locked"), None),
            Effect::ShowError(details) => {
                eprintln!("a Quick Save failed: {details}");
                inform(
                    window,
                    tr_key("save.quick"),
                    &tr("save.failed"),
                    Some(&details),
                );
            }
            Effect::Toast(key) => toast::show(&tr(key), None, NOTICE_SECONDS),
            Effect::AnimateNewStation(id) => {
                let made = self.quick_save.borrow_mut().made.take();
                if let Some((save, parent)) = made.filter(|(save, _)| save.id == id) {
                    tab.show_new_save(save, parent);
                }
            }
            Effect::Refresh => tab.refresh(),
        }
    }

    /// Scans the unsaved changes on the tab's worker, and looks for Claude there too when the
    /// AI names saves.
    fn run_scan(&self, run: u64, tab: &ProjectTab) {
        let (large_file_mb, wants_ai) = {
            let settings = self.settings.borrow();
            (settings.large_file_mb, settings.naming == NamingMode::Ai)
        };
        tab.submit(move |p| {
            let scanned = scan_for_save(p, large_file_mb);
            let claude = if wants_ai { locate_claude() } else { None };
            Box::new(move || with_app(|app| app.scanned(run, scanned, claude)))
        });
    }

    fn scanned(&self, run: u64, scanned: Scanned, claude: Option<PathBuf>) {
        let (current, tab) = {
            let mut quick_save = self.quick_save.borrow_mut();
            if quick_save.run == run {
                quick_save.claude = claude;
            }
            (quick_save.run == run, quick_save.tab)
        };
        match scanned {
            Scanned::Done(ev) => self.quick_save_event(run, ev),
            Scanned::ReadOnly if current => {
                // The refresh shows why, and turns Quick Save off.
                self.abandon_quick_save();
                with_tab(tab, |tab| tab.refresh());
            }
            Scanned::ReadOnly => {}
        }
    }

    /// The identity dialog closed: writes who is saving on the tab's worker.
    fn identity_answered(&self, run: u64, answer: Option<(String, String)>) {
        let Some((name, email)) = answer else {
            self.quick_save_event(run, QsEvent::IdentityCancelled);
            return;
        };
        let Some(tab) = self.tab(self.quick_save.borrow().tab) else {
            self.abandon_quick_save();
            return;
        };
        tab.submit(move |p| {
            let ev = match set_identity_if_missing(&p.repo().exe, &name, &email) {
                Ok(()) => QsEvent::IdentityGiven,
                Err(error) => QsEvent::SaveDone(Err(save_error(error))),
            };
            back_to(run, ev)
        });
    }

    /// Reads what the save would record (minus `excluded`) on the tab's worker, then asks the AI
    /// for a name on a thread of its own.
    fn run_ai(&self, run: u64, tab: &ProjectTab, excluded: Vec<String>) {
        let cancel = CancelToken::default();
        let claude = {
            let mut quick_save = self.quick_save.borrow_mut();
            quick_save.cancel = Some(cancel.clone());
            quick_save.claude.clone()
        };
        let Some(claude) = claude else {
            self.quick_save_event(run, QsEvent::AiDone(Err(NamingError::NotInstalled)));
            return;
        };
        let (model, timeout) = {
            let settings = self.settings.borrow();
            let timeout = Duration::from_secs(settings.ai_timeout_secs);
            (settings.ai_model.clone(), timeout)
        };
        let lang = self.lang;
        tab.submit(move |p| match naming_input(p, &excluded) {
            Ok(input) => {
                let request = NamingRequest {
                    stat: input.stat,
                    patch: input.patch,
                    lang,
                    model,
                    timeout,
                };
                ask_ai(run, claude, request, cancel);
                Box::new(|| {})
            }
            Err(error) => {
                let detail = format!("the changes could not be read: {error}");
                let failed = NamingError::Failed { code: None, detail };
                back_to(run, QsEvent::AiDone(Err(failed)))
            }
        });
    }

    /// Saves everything on the tab's worker, and reads the new save back for the timeline.
    fn run_commit(&self, run: u64, tab: &ProjectTab, name: String, checked: Vec<String>) {
        let large_file_mb = self.settings.borrow().large_file_mb;
        tab.submit(move |p| {
            let result = commit_all(p, &name, &checked, large_file_mb);
            let made = result.as_ref().ok().and_then(|id| new_save(p, id));
            Box::new(move || with_app(|app| app.committed(run, result, made)))
        });
    }

    fn committed(
        &self,
        run: u64,
        result: Result<SaveId, SaveError>,
        made: Option<(SaveState, Option<SaveId>)>,
    ) {
        {
            let mut quick_save = self.quick_save.borrow_mut();
            if quick_save.run != run {
                return;
            }
            quick_save.made = made;
        }
        self.quick_save_event(run, QsEvent::SaveDone(result));
    }

    /// Ends the Quick Save under way without finishing it (its tab closed, or its project can
    /// no longer be saved to). What it waited for is dropped when it comes back, and what was to
    /// run after it never runs.
    fn abandon_quick_save(&self) {
        let abandoned = {
            let mut quick_save = self.quick_save.borrow_mut();
            if let Some(cancel) = quick_save.cancel.take() {
                cancel.cancel();
            }
            let next = QuickSaveRun::idle(quick_save.run + 1);
            std::mem::replace(&mut *quick_save, next)
        };
        // Dropped outside the borrow: what the continuation holds may do anything when dropped.
        drop(abandoned);
        ai_spinner::hide();
    }
}

/// What a step on the project's thread gives back: `ev`, for the Quick Save `run`.
fn back_to(run: u64, ev: QsEvent) -> Box<dyn FnOnce() + Send> {
    Box::new(move || with_app(|app| app.quick_save_event(run, ev)))
}

/// The safety scan before a Quick Save, on the project's thread: the flags, whether who is
/// saving is unknown, and the changes the user is shown (`checked`).
fn scan_for_save(p: &Project, large_file_mb: u64) -> Scanned {
    let snap = snapshot(p);
    if snap.condition != RepoCondition::Normal {
        return Scanned::ReadOnly;
    }
    if snap.changes.is_empty() {
        return Scanned::Done(QsEvent::SaveDone(Err(SaveError::NothingToSave)));
    }
    let found =
        scan(p, &snap.changes, large_file_mb).and_then(|flags| Ok((flags, identity_missing(p)?)));
    Scanned::Done(match found {
        Ok((flags, identity_missing)) => QsEvent::ScanDone {
            flags,
            identity_missing,
            checked: snap.changes.into_iter().map(|change| change.path).collect(),
        },
        Err(error) => QsEvent::SaveDone(Err(save_error(error))),
    })
}

/// Asks `claude` for a name on a thread of its own (never the UI thread, nor Raven's job pool),
/// and posts the answer to the Quick Save `run`.
fn ask_ai(run: u64, claude: PathBuf, request: NamingRequest, cancel: CancelToken) {
    let spawned = std::thread::Builder::new()
        .name("savestates-ai".to_owned())
        .spawn(move || {
            let result = if cancel.is_cancelled() {
                Err(NamingError::Cancelled)
            } else {
                generate_name(&claude, &request, &cancel)
            };
            if let Err(NamingError::Failed { code, detail }) = &result {
                // Only for the log: the user sees a short note, never this.
                let detail: String = detail.chars().take(500).collect();
                eprintln!("the AI gave no name (exit code {code:?}): {detail}");
            }
            task::post(move || with_app(|app| app.quick_save_event(run, QsEvent::AiDone(result))));
        });
    if let Err(error) = spawned {
        let detail = format!("the AI thread could not start: {error}");
        task::post(move || {
            let failed = NamingError::Failed { code: None, detail };
            with_app(|app| app.quick_save_event(run, QsEvent::AiDone(Err(failed))));
        });
    }
}

/// The save just made (`id`) and the one before it, read back for the timeline.
fn new_save(p: &Project, id: &SaveId) -> Option<(SaveState, Option<SaveId>)> {
    let mut page = timeline_page(p, 0, 2).ok()?.into_iter();
    let save = page.next().filter(|save| &save.id == id)?;
    Some((save, page.next().map(|before| before.id)))
}

/// A failed scan, decision or identity write, told to the flow as a failed save.
fn save_error(error: CoreError) -> SaveError {
    match error {
        CoreError::Git(error) => SaveError::Git(error),
        CoreError::Io(error) => SaveError::Git(GitError::Io(error)),
        other => SaveError::Git(GitError::Io(io::Error::other(other))),
    }
}

/// The key of a project's timeline zoom in the settings.
fn zoom_key(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

/// Loads the app's theme over Raven's dark one, with the cyan accent.
fn load_theme() {
    if let Err(error) = ui::theme::load_xml(&ui_files::source("theme.xml")) {
        eprintln!("the theme could not be loaded: {error}");
    }
    ui::theme::set_dark(true);
    let Rgba(r, g, b, _) = theme::LINE;
    ui::theme::set_accent(r, g, b);
}

/// Loads the translations and picks `lang`, with PT-BR for any text a language lacks.
fn load_translations(lang: Lang) {
    if let Err(error) = locale::load_csv(TRANSLATIONS_CSV) {
        eprintln!("the translations could not be loaded: {error}");
    }
    locale::set_fallback("pt-BR");
    locale::set_language(lang.code());
}

/// Runs a one-off job that blocks (git) on a thread of its own, never on the UI thread nor on
/// Raven's small job pool. False when the thread could not start (the user was told).
fn run_in_background(job: impl FnOnce() + Send + 'static) -> bool {
    let spawned = std::thread::Builder::new()
        .name("savestates-open".to_owned())
        .spawn(job);
    if let Err(error) = &spawned {
        eprintln!("a background job could not start: {error}");
        toast::show(&tr("project.open_failed"), None, NOTICE_SECONDS);
    }
    spawned.is_ok()
}
