//! The app: startup, the main window, opening projects into tabs, and what is remembered between
//! runs.
//!
//! Everything here runs on Raven's UI thread. Work that blocks (git) runs on other threads and
//! comes back through `task::post`; a posted closure cannot hold the app (it is not `Send`), so it
//! finds it again with [`with_app`] and a tab with [`with_tab`].

mod load_glue;
mod quick_save_glue;
mod recovery_glue;
mod settings_glue;

use std::cell::{Cell, OnceCell, RefCell};
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use savestates_core::settings::{self, Settings, WindowGeom};
use savestates_core::{CoreError, OpenOutcome, Project, open_project, start_saving_folder};
use savestates_git::GitExe;
use savestates_i18n::{Lang, TRANSLATIONS_CSV, resolve, startup_failure_text, system_language};
use savestates_ui_raven::{
    self as ui, Backend, Key, Rgba, Widget, Window, bind_action, dialog, locale, tabs as tab_view,
    task, tr, tr_fmt, widget_from_xml,
};

use crate::dialogs::{confirm, inform};
use crate::start_screen::{GIT_DOWNLOAD_URL, StartScreen};
use crate::tabs::{
    ProjectTab, RestoreState, TabButton, TabId, TabOp, TabsState, same_folder, same_spelling,
};
use crate::{theme, toast, tr_key, ui_files};
use quick_save_glue::QuickSaveRun;

/// How long a toast that only informs stays up.
const NOTICE_SECONDS: f64 = 8.0;
/// How long a toast that offers to undo stays up.
const UNDO_SECONDS: f64 = 10.0;
/// A window at or beyond these coordinates is minimized (Windows parks it at -32000).
const OFF_SCREEN: i32 = -10_000;
/// While the last run's tabs reopen, the start screen shows if none is back after this long.
const RESTORE_GRACE_SECONDS: f64 = 0.5;
/// The answer "Open" to the question asked before opening a bigger project.
const OPEN: i32 = 1;

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
    /// The language shown; the Settings dialog changes it while the app runs.
    lang: Cell<Lang>,
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
    /// Counts the naming modes chosen in Settings: a search for Claude that comes back after
    /// another choice was made is dropped.
    naming_asked: Cell<u64>,
}

impl App {
    /// Starts the app and runs it until its window closes. Returns the process exit code.
    pub fn run() -> i32 {
        let settings_path = settings::default_path();
        // A release build has no console: diagnostics and panics go to a file a helper can read.
        crate::log::install(crate::log::next_to(&settings_path));
        let settings = settings::load(&settings_path);
        let lang = resolve(settings.language, &system_language());
        if let Err(error) = ui::init(Backend::Vulkan) {
            log!("the graphics could not start: {error}");
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
                log!("the main window could not be built: {error}");
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
        let tab_view = find("tabs")?;
        tab_view.set_context_menu(widget_from_xml(&ui_files::source("menu_tabs.xml"))?);
        toast::install(window);
        Ok(Rc::new(App {
            window,
            settings_path,
            settings: RefCell::new(settings),
            lang: Cell::new(lang),
            git,
            tab_view,
            start_host,
            open_button: find("open_project")?,
            start: RefCell::new(start),
            tabs: RefCell::default(),
            restore: RefCell::default(),
            pages: RefCell::default(),
            next_id: Cell::new(1),
            quick_save: RefCell::new(QuickSaveRun::idle(0)),
            naming_asked: Cell::new(0),
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
        bind_action("app.settings", |_| with_app(|app| app.open_settings()));
        bind_action("app.recovery", |_| {
            with_app(|app| app.open_recovery_active())
        });
        self.window
            .register_shortcut(Key::Char('S'), true, false, "app.quick_save");
        bind_action("app.escape", |_| with_app(|app| app.escape()));
        self.window
            .register_shortcut(Key::Escape, false, false, "app.escape");
        self.window.on_key_down(|key, ctrl, shift| {
            let mut taken = false;
            with_app(|app| taken = app.key_down(key, ctrl, shift));
            taken
        });
        for action in [
            "tab.jump_to_now",
            "tab.quick_save",
            "tab.check_again",
            "tab.remove",
            "tab.back_to_now",
            "tab.bring_back_everything",
            "tab.bring_back",
            "tab.throw_away",
            "tab.throw_away_file",
            "tab.load",
            "tab.load_here",
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
        let started = open_in_background(move || {
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
    /// projects. A folder inside a bigger project asks first.
    fn open_folder(&self, folder: PathBuf) {
        let Some(git) = self.git.clone() else { return };
        open_in_background(move || {
            // Resolved here rather than on the UI thread (a network folder can be slow), to
            // compare with the top folder of the project it opens.
            let folder = dunce::canonicalize(&folder).unwrap_or(folder);
            let outcome = open_project(&git, &folder);
            task::post(move || with_app(|app| app.opened(folder, outcome, Opening::ByUser)));
        });
    }

    fn opened(&self, folder: PathBuf, outcome: Result<OpenOutcome, CoreError>, how: Opening) {
        match outcome {
            Ok(OpenOutcome::Opened(project)) => match how {
                Opening::ByUser if opens_a_bigger_project(&folder, project.root()) => {
                    self.ask_open_bigger(project);
                }
                Opening::ByUser => self.show_chosen(project),
                Opening::Restored { was_active } => {
                    self.add_project(project, was_active);
                }
            },
            Ok(OpenOutcome::NotARepo) => self.ask_start_saving(folder),
            Ok(OpenOutcome::Missing) => self.add_missing(folder, how.activates()),
            Err(error) => {
                log!("{} could not be opened: {error}", folder.display());
                if how == Opening::ByUser {
                    self.tell_failure(
                        tr_key("project.open"),
                        &tr("project.open_failed"),
                        &folder,
                        &error,
                    );
                } else {
                    toast::show(tr_key("project.open_failed"), None, NOTICE_SECONDS);
                }
            }
        }
    }

    /// Shows a project the user asked for in the active tab, and puts it first in the recent
    /// projects.
    fn show_chosen(&self, project: Project) {
        self.remember(project.root());
        self.add_project(project, true);
    }

    /// The folder the user chose is inside a bigger project: asks before opening all of it.
    fn ask_open_bigger(&self, project: Project) {
        let root = project.root().display().to_string();
        confirm(
            self.window,
            tr_key("project.open"),
            &tr_fmt("project.open_bigger", &[&root]),
            &[(tr_key("common.open"), OPEN), (tr_key("common.cancel"), 0)],
            move |answer| {
                if answer == OPEN {
                    // Once the question is gone: it is still closing now.
                    task::post(move || with_app(|app| app.show_chosen(project)));
                }
            },
        );
    }

    /// Tells the user that something they asked for about `folder` failed: the title of
    /// `title_key`, the text `body` (already translated) with the folder, and the error under
    /// "Details", for someone helping them.
    fn tell_failure(&self, title_key: &str, body: &str, folder: &Path, error: &dyn Display) {
        let body = format!("{body}\n\n{}", folder.display());
        inform(self.window, title_key, &body, Some(&error.to_string()));
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
        open_in_background(move || {
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
                        log!("{} could not start saving: {error}", folder.display());
                        app.tell_failure(
                            tr_key("project.start_saving"),
                            &tr("project.start_saving_failed"),
                            &folder,
                            &error,
                        );
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
                let day_px = self
                    .settings
                    .borrow()
                    .timeline_day_px
                    .get(&day_px_key(&root))
                    .map(|&px| f64::from(px));
                let id = self.new_id();
                match ProjectTab::open(id, project, self.lang.get(), day_px, self.window) {
                    Ok(tab) => {
                        self.insert_tab(tab.clone());
                        tab.prune_recovery();
                        tab.sweep();
                        tab.refresh();
                        Some(tab)
                    }
                    Err(error) => {
                        self.tabs.borrow_mut().close(i);
                        log!("the tab could not be built: {error}");
                        toast::show(tr_key("project.open_failed"), None, NOTICE_SECONDS);
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
                    log!("the tab could not be built: {error}");
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
        self.quick_save_tab_closed(tab.id);
        if let Some(day_px) = tab.day_px() {
            self.settings
                .borrow_mut()
                .timeline_day_px
                .insert(day_px_key(&tab.root), day_px as f32);
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
            TabButton::CheckAgain => {
                tab.sweep();
                tab.refresh();
            }
            TabButton::Remove => self.remove_missing(i),
            TabButton::BackToNow => tab.back_to_now(),
            TabButton::BringBackEverything => tab.bring_back_everything(self.window),
            TabButton::BringBack(which) => tab.bring_back(self.window, Some(which)),
            TabButton::BringBackThisFile => tab.bring_back(self.window, None),
            TabButton::ThrowAway => self.throw_away(&tab, false),
            TabButton::ThrowAwayFile => self.throw_away(&tab, true),
            TabButton::Load => self.load_peeked(&tab),
            TabButton::LoadHere => self.load_here(&tab),
        }
    }

    /// The main window, for the dialogs shown in it.
    pub fn window(&self) -> Window {
        self.window
    }

    fn refresh_active(&self) {
        if let Some(tab) = self.active_tab() {
            tab.refresh();
        }
    }

    /// A key pressed in the window, before its widgets and shortcuts see it: true takes it.
    ///
    /// Esc with the pointer over the active tab's timeline is the timeline's (spec §4.6), even
    /// while a text input such as peek's file filter has the keyboard, which keeps the window
    /// shortcut from firing: it closes a group's list or ends the temporary zoom. When the
    /// timeline has nothing to go back from, or the pointer is elsewhere, Esc goes on as usual,
    /// to the text input or to the `app.escape` shortcut, so it is never handled twice. Not
    /// while a dialog, a menu or a drop-down list is open: Esc is theirs.
    fn key_down(&self, key: Key, ctrl: bool, shift: bool) -> bool {
        key == Key::Escape
            && !ctrl
            && !shift
            && !self.window.has_popup()
            && self
                .active_tab()
                .is_some_and(|tab| tab.escape_under_pointer())
    }

    /// Esc, anywhere in the window: the active tab's timeline closes a group's list or ends its
    /// temporary zoom. Not while a dialog, a menu or a drop-down list is open: Esc is theirs
    /// (Raven closes them on Esc without firing the shortcut, so this is only a safeguard).
    fn escape(&self) {
        if self.window.has_popup() {
            return;
        }
        if let Some(tab) = self.active_tab() {
            tab.escape();
        }
    }

    /// The tab shown.
    fn active_tab(&self) -> Option<Rc<ProjectTab>> {
        let active = self.tabs.borrow().active();
        active.and_then(|i| self.pages.borrow().get(i).cloned())
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
    /// timeline zoom (the width of a day). Only ever called on the UI thread, one write at a
    /// time.
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
                if let Some(day_px) = tab.day_px() {
                    let key = day_px_key(&tab.root);
                    settings.timeline_day_px.insert(key, day_px as f32);
                }
            }
        }
        settings.window = self.geometry(settings.window);
        if let Err(error) = settings::save(&self.settings_path, &settings) {
            log!("the settings could not be saved: {error}");
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

/// Whether opening the folder the user chose, `chosen`, would open a bigger project around it,
/// whose top folder is `root`: then the user is asked first (someone with a project in their
/// whole user folder by accident would otherwise open all of it). Both paths are resolved
/// already, so the file system is not asked again.
fn opens_a_bigger_project(chosen: &Path, root: &Path) -> bool {
    !same_spelling(chosen, root)
}

/// The key of a project's timeline zoom (`timeline_day_px`) in the settings.
fn day_px_key(root: &Path) -> String {
    root.to_string_lossy().into_owned()
}

/// Loads the app's theme over Raven's dark one, with the cyan accent.
fn load_theme() {
    if let Err(error) = ui::theme::load_xml(&ui_files::source("theme.xml")) {
        log!("the theme could not be loaded: {error}");
    }
    ui::theme::set_dark(true);
    let Rgba(r, g, b, _) = theme::LINE;
    ui::theme::set_accent(r, g, b);
}

/// Loads the translations and picks `lang`, with PT-BR for any text a language lacks.
fn load_translations(lang: Lang) {
    if let Err(error) = locale::load_csv(TRANSLATIONS_CSV) {
        log!("the translations could not be loaded: {error}");
    }
    locale::set_fallback("pt-BR");
    locale::set_language(lang.code());
}

/// Runs a one-off job that blocks (git, or looking for a program) on a thread of its own,
/// never on the UI thread nor on Raven's small job pool. False when the thread could not start.
fn run_in_background(job: impl FnOnce() + Send + 'static) -> bool {
    let spawned = std::thread::Builder::new()
        .name("savestates-job".to_owned())
        .spawn(job);
    if let Err(error) = &spawned {
        log!("a background job could not start: {error}");
    }
    spawned.is_ok()
}

/// [`run_in_background`] for a job that opens a project: when it cannot start, the user is
/// told that the project could not be opened.
fn open_in_background(job: impl FnOnce() + Send + 'static) -> bool {
    let started = run_in_background(job);
    if !started {
        toast::show(tr_key("project.open_failed"), None, NOTICE_SECONDS);
    }
    started
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_subfolder_of_a_project_asks_before_opening_the_whole_project() {
        let root = Path::new(r"C:\Users\ana");
        assert!(opens_a_bigger_project(
            Path::new(r"C:\Users\ana\Documentos\site"),
            root
        ));
        assert!(opens_a_bigger_project(
            Path::new(r"C:\Users\ana\site"),
            root
        ));
    }

    #[test]
    fn the_top_folder_itself_opens_without_asking() {
        let root = Path::new(r"C:\Users\ana\site");
        assert!(!opens_a_bigger_project(root, root));
        // Spelled another way: a trailing separator, or (on Windows) other letter case.
        assert!(!opens_a_bigger_project(
            Path::new(r"C:\Users\ana\site\"),
            root
        ));
        if cfg!(windows) {
            assert!(!opens_a_bigger_project(
                Path::new(r"c:\users\ANA\Site"),
                root
            ));
        }
        // A whole drive that is a project.
        assert!(!opens_a_bigger_project(
            Path::new(r"D:\"),
            Path::new(r"D:\")
        ));
    }
}
