//! The timelines' own actions (spec §5.3, §7): the menu of a lane's name, renaming, archiving
//! with Desfazer, the list of archived timelines with Restaurar and Apagar…, and deleting with
//! Desfazer.
//!
//! The git work runs on the tab's worker and every operation ends with a refresh. A project that
//! can only be looked at refuses all of it, with the reason its banner gives.

use std::cell::{Cell, RefCell};

use savestates_core::{
    ArchiveError, ArchivedTimeline, CoreError, archive_timeline, delete_timeline, list_archived,
    rename_timeline, restore_deleted, unarchive_timeline,
};
use savestates_ui_raven::{Widget, bind_action, tr_fmt, tr_plural, widget_from_xml};

use super::{App, NOTICE_SECONDS, UNDO_SECONDS, with_app, with_tab};
use crate::dialogs::archived::{self, ArchivedChoice};
use crate::dialogs::{confirm_destructive, inform, rename};
use crate::tabs::TabId;
use crate::timeline::model::LaneData;
use crate::worker::when_normal;
use crate::{toast, tr_key, ui_files};

/// The answer "Apagar" to the question asked first.
const DELETE: i32 = 1;

/// The lane the menu was opened on.
struct Target {
    tab: TabId,
    lane: LaneData,
}

thread_local! {
    static MENU: RefCell<Option<Widget>> = const { RefCell::new(None) };
    static TARGET: RefCell<Option<Target>> = const { RefCell::new(None) };
    static BOUND: Cell<bool> = const { Cell::new(false) };
}

/// Binds the lane menu's actions, once.
pub(super) fn bind_actions() {
    if BOUND.replace(true) {
        return;
    }
    bind_action("lane.rename", |_| {
        if let Some(target) = TARGET.take() {
            with_app(|app| app.ask_new_name(target.tab, target.lane));
        }
    });
    bind_action("lane.archive", |_| {
        if let Some(target) = TARGET.take() {
            with_app(|app| app.archive_lane(target.tab, &target.lane));
        }
    });
}

/// The menu of a lane's name, built once.
fn menu() -> Option<Widget> {
    MENU.with_borrow_mut(|menu| {
        if menu.is_none() {
            match widget_from_xml(&ui_files::source("menu_lane.xml")) {
                Ok(built) => *menu = Some(built),
                Err(error) => log!("the lane menu could not be built: {error}"),
            }
        }
        *menu
    })
}

impl App {
    /// A right click on the name of lane `lane` of the tab `tab`, at (`x`, `y`) in its timeline.
    pub fn open_lane_menu(&self, tab: TabId, lane: usize, x: f32, y: f32) {
        let Some(tab_ref) = self.tab(tab) else { return };
        let Some(data) = tab_ref.lane(lane).filter(|data| data.branch.is_some()) else {
            return;
        };
        let (Some(menu), Some((wx, wy))) = (menu(), tab_ref.timeline_to_window(x, y)) else {
            return;
        };
        TARGET.set(Some(Target { tab, lane: data }));
        self.window.open_menu(menu, wx, wy);
    }

    /// A double click on the name of lane `lane` of the tab `tab`.
    pub fn rename_lane(&self, tab: TabId, lane: usize) {
        let Some(data) = self.tab(tab).and_then(|t| t.lane(lane)) else {
            return;
        };
        if data.branch.is_some() {
            self.ask_new_name(tab, data);
        }
    }

    /// Asks for the new name of `lane`, then renames it.
    fn ask_new_name(&self, tab: TabId, lane: LaneData) {
        let (Some(branch), Some(tab_ref)) = (lane.branch.clone(), self.tab(tab)) else {
            return;
        };
        if let Some(why) = tab_ref.restore_off() {
            inform(self.window, tr_key("rename.title"), &why, None);
            return;
        }
        rename::show(self.window, &lane.label, move |answer| {
            let Some(name) = answer else { return };
            with_tab(tab, |t| {
                t.submit(move |p| {
                    let result = when_normal(p, |p| rename_timeline(p, &branch, &name));
                    Box::new(move || {
                        if let Some(Err(error)) = result {
                            failed(&error);
                        }
                        with_tab(tab, |t| t.refresh());
                    })
                });
            });
        });
    }

    /// "Arquivar" on `lane`: says why not when it cannot be, else archives it, offering Desfazer.
    fn archive_lane(&self, tab: TabId, lane: &LaneData) {
        let (Some(branch), Some(tab_ref)) = (lane.branch.clone(), self.tab(tab)) else {
            return;
        };
        let refusal = tab_ref.restore_off().or_else(|| tab_ref.lane_refusal(lane));
        if let Some(text) = refusal {
            inform(self.window, tr_key("lane.cant_archive_title"), &text, None);
            return;
        }
        tab_ref.submit(move |p| {
            let result = when_normal(p, |p| archive_timeline(p, &branch));
            Box::new(move || {
                with_tab(tab, |t| t.refresh());
                match result {
                    Some(Ok(())) => {
                        let undo: Box<dyn FnOnce()> = Box::new(move || {
                            unarchive(tab, branch);
                        });
                        toast::show(
                            tr_key("lane.archived"),
                            Some((tr_key("common.undo"), undo)),
                            UNDO_SECONDS,
                        );
                    }
                    Some(Err(error)) => {
                        with_app(|app| app.tell_refused(&error));
                    }
                    None => {}
                }
            })
        });
    }

    /// An archive or delete the core refused after all (the project changed since the menu
    /// opened): says why when it knows.
    fn tell_refused(&self, error: &ArchiveError) {
        let text = match error {
            ArchiveError::MainLine => tr_key("lane.archive_main"),
            ArchiveError::Current => tr_key("lane.archive_current"),
            ArchiveError::Elsewhere(path) => {
                let text = tr_fmt("lane.elsewhere", &[&path.display().to_string()]);
                inform(self.window, tr_key("lane.cant_archive_title"), &text, None);
                return;
            }
            ArchiveError::NotFound | ArchiveError::Core(_) => {
                failed(error);
                return;
            }
        };
        inform(
            self.window,
            tr_key("lane.cant_archive_title"),
            &savestates_ui_raven::tr(text),
            None,
        );
    }

    /// "Linhas do tempo arquivadas…" in a context menu: the list of the tab shown.
    pub(super) fn open_archived_active(&self) {
        let Some(tab) = self.active_tab() else { return };
        if tab.is_missing() {
            return;
        }
        let id = tab.id;
        tab.submit(move |p| {
            let listed = list_archived(p);
            Box::new(move || with_app(|app| app.archived_listed(id, listed)))
        });
    }

    fn archived_listed(&self, id: TabId, listed: Result<Vec<ArchivedTimeline>, CoreError>) {
        let Some(tab) = self.tab(id) else { return };
        let Some(lang) = tab.lang() else { return };
        let rows = match listed {
            Ok(rows) => rows,
            Err(error) => {
                log!("the archived timelines could not be read: {error}");
                toast::show(tr_key("lane.failed"), None, NOTICE_SECONDS);
                return;
            }
        };
        let kept = rows.clone();
        let off = tab.restore_off();
        archived::show(
            self.window,
            &tab.title(),
            rows,
            lang,
            off.as_deref(),
            move |choice| match choice {
                ArchivedChoice::Restore(branch) => unarchive(id, branch),
                ArchivedChoice::Delete(branch) => {
                    if let Some(row) = kept.into_iter().find(|row| row.branch == branch) {
                        with_app(|app| app.ask_delete(id, &row));
                    }
                }
            },
        );
    }

    /// "Apagar…" on an archived timeline: asks first, then deletes it, offering Desfazer.
    fn ask_delete(&self, tab: TabId, row: &ArchivedTimeline) {
        let name = row.display_name.as_deref().unwrap_or(&row.branch);
        let count = i64::try_from(row.own_saves).unwrap_or(i64::MAX);
        let body = tr_fmt("delete.body", &[name, &tr_plural("timeline.count", count)]);
        let branch = row.branch.clone();
        confirm_destructive(
            self.window,
            tr_key("delete.title"),
            &body,
            (tr_key("delete.confirm"), DELETE),
            move |answer| {
                if answer == DELETE {
                    run_delete(tab, branch);
                }
            },
        );
    }
}

/// Takes the timeline `branch` out of the archive, on the worker of the tab `tab`.
fn unarchive(tab: TabId, branch: String) {
    with_tab(tab, |t| {
        t.submit(move |p| {
            let result = when_normal(p, |p| unarchive_timeline(p, &branch));
            Box::new(move || {
                if let Some(Err(error)) = result {
                    failed(&error);
                }
                with_tab(tab, |t| t.refresh());
            })
        });
    });
}

/// Deletes the archived timeline `branch`, on the worker of the tab `tab`.
fn run_delete(tab: TabId, branch: String) {
    with_tab(tab, |t| {
        t.submit(move |p| {
            let result = when_normal(p, |p| delete_timeline(p, &branch));
            Box::new(move || {
                match result {
                    Some(Ok(deleted)) => {
                        let ref_name = deleted.ref_name;
                        let undo: Box<dyn FnOnce()> = Box::new(move || {
                            undo_delete(tab, ref_name);
                        });
                        toast::show(
                            tr_key("delete.done"),
                            Some((tr_key("common.undo"), undo)),
                            UNDO_SECONDS,
                        );
                    }
                    Some(Err(error)) => failed(&error),
                    None => {}
                }
                with_tab(tab, |t| t.refresh());
            })
        });
    });
}

/// Desfazer on a deleted timeline: brings it back (archived, as it was).
fn undo_delete(tab: TabId, ref_name: String) {
    with_tab(tab, |t| {
        t.submit(move |p| {
            let result = when_normal(p, |p| restore_deleted(p, &ref_name));
            Box::new(move || {
                if let Some(Err(error)) = result {
                    log!("undoing the deletion failed: {error}");
                    toast::show(tr_key("lane.failed"), None, NOTICE_SECONDS);
                }
                with_tab(tab, |t| t.refresh());
            })
        });
    });
}

/// An operation failed for a reason the user cannot do much about: logs it and says so.
fn failed(error: &dyn std::fmt::Display) {
    log!("a timeline operation failed: {error}");
    toast::show(tr_key("lane.failed"), None, NOTICE_SECONDS);
}
