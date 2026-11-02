//! The app: startup, the main window, opening projects into tabs, and what is remembered between
//! runs.
//!
//! Everything here runs on Raven's UI thread. Work that blocks (git) runs on other threads and
//! comes back through `task::post`; a posted closure cannot hold the app (it is not `Send`), so it
//! finds it again with [`with_app`] and a tab with [`with_tab`].

use std::cell::{Cell, OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use savestates_core::settings::{self, Settings, WindowGeom};
use savestates_core::{CoreError, OpenOutcome, Project, open_project, start_saving_folder};
use savestates_git::GitExe;
use savestates_i18n::{Lang, TRANSLATIONS_CSV, resolve, startup_failure_text, system_language};
use savestates_ui_raven::{
    self as ui, Backend, Rgba, Widget, Window, bind_action, dialog, locale, tabs as tab_view, task,
    tr,
};

use crate::dialogs::confirm;
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
        for action in [
            "tab.jump_to_now",
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

    /// Offers the first save of a folder that just started saving.
    ///
    /// Task 17 implements it: a Quick Save whose name is prefilled with
    /// `tr("save.first_name")` ("Começo do projeto" / "Project start"), which the user can edit.
    fn offer_first_save(&self, tab: &ProjectTab) {
        let _ = tab;
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

    /// Quick Saves the project of the tab `tab`, then runs `continuation` once the save is done
    /// ("Quick Save first", before bringing everything back over unsaved changes).
    ///
    /// Task 17 implements it: start the Quick Save flow on the tab and, on Done, run the
    /// continuation (here `bring_back_everything(.., RequireClean)`). It does nothing until then.
    pub fn quick_save_then(&self, tab: TabId, continuation: Box<dyn FnOnce()>) {
        let _ = (tab, continuation);
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
