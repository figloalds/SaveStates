//! Throw away and the Recovery list (spec §6.8.2).
//!
//! "Throw away changes" (everything, or one file from the changes' context menu) asks first,
//! then the core sets the work aside in the Recovery list before removing it, and a toast offers
//! to undo for 10 seconds. The Recovery list shows what was set aside in the last 30 days and
//! brings one entry back. The git work runs on the tab's worker, and every operation ends with
//! a refresh.

use savestates_core::{
    CoreError, Project, RecoveryEntry, RepoCondition, Scope, list_recovery, restore_recovery,
    snapshot, throw_away,
};
use savestates_ui_raven::{tr, tr_fmt};

use super::{App, NOTICE_SECONDS, with_app, with_tab};
use crate::bring_back_ui::offer_undo;
use crate::dialogs::{confirm, recovery};
use crate::tabs::{ProjectTab, TabId};
use crate::{toast, tr_key};

/// The answer "Throw away" to the question asked first.
const DISCARD: i32 = 1;

impl App {
    /// "Throw away changes" on `tab`: everything, or with `file` the selected unsaved change.
    /// Nothing happens when nothing may be thrown away (see [`ProjectTab::throw_away_scope`]).
    pub(super) fn throw_away(&self, tab: &ProjectTab, file: bool) {
        let Some(scope) = tab.throw_away_scope(file) else {
            return;
        };
        let body = match &scope {
            Scope::All => tr("throw_away.confirm"),
            Scope::File(path) => tr_fmt("throw_away.confirm_file", &[path]),
        };
        let id = tab.id;
        confirm(
            self.window,
            tr_key("throw_away.action"),
            &body,
            &[
                (tr_key("throw_away.discard"), DISCARD),
                (tr_key("common.cancel"), 0),
            ],
            move |answer| {
                if answer == DISCARD {
                    run_throw_away(id, scope);
                }
            },
        );
    }

    /// "Recovery…" in a context menu: the Recovery list of the tab shown.
    pub(super) fn open_recovery_active(&self) {
        if let Some(tab) = self.active_tab() {
            self.open_recovery(&tab);
        }
    }

    /// Reads the Recovery list of `tab` on its worker, then shows it.
    fn open_recovery(&self, tab: &ProjectTab) {
        if tab.is_missing() {
            return;
        }
        let id = tab.id;
        tab.submit(move |p| {
            let listed = list_recovery(p);
            Box::new(move || with_app(|app| app.recovery_listed(id, listed)))
        });
    }

    fn recovery_listed(&self, id: TabId, listed: Result<Vec<RecoveryEntry>, CoreError>) {
        let Some(tab) = self.tab(id) else { return };
        let Some(lang) = tab.lang() else { return };
        let entries = match listed {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!("the recovery list could not be read: {error}");
                toast::show(tr_key("recovery.list_failed"), None, NOTICE_SECONDS);
                return;
            }
        };
        let restore_off = tab.restore_off();
        recovery::show(
            self.window,
            &tab.title(),
            entries,
            lang,
            restore_off.as_deref(),
            move |ref_name| run_restore(id, ref_name),
        );
    }
}

/// Throws away `scope` in the project of the tab `tab`, on its worker.
fn run_throw_away(tab: TabId, scope: Scope) {
    with_tab(tab, |t| {
        t.submit(move |p| {
            let result = when_normal(p, |p| throw_away(p, &scope));
            Box::new(move || thrown_away(tab, result))
        });
    });
}

/// Throwing away is done: the toast offers to undo it, and the tab refreshes. `None`: the
/// project could only be looked at by then, and nothing was touched (the refresh says why).
fn thrown_away(tab: TabId, result: Option<Result<Option<RecoveryEntry>, CoreError>>) {
    match result {
        Some(Ok(Some(entry))) => offer_undo(tab, tr_key("throw_away.done"), entry),
        // Nothing to throw away, or no save to go back to: the refresh shows how things are.
        Some(Ok(None) | Err(CoreError::NoSavesYet)) | None => {}
        Some(Err(error)) => {
            eprintln!("throwing away failed: {error}");
            toast::show(tr_key("throw_away.failed"), None, NOTICE_SECONDS);
        }
    }
    with_tab(tab, |t| t.refresh());
}

/// Brings the Recovery entry `ref_name` back into the project of the tab `tab`, on its worker.
fn run_restore(tab: TabId, ref_name: String) {
    with_tab(tab, |t| {
        t.submit(move |p| {
            let result = when_normal(p, |p| restore_recovery(p, &ref_name));
            Box::new(move || restored(tab, result))
        });
    });
}

/// An entry was brought back: says so, offering to undo when the unsaved changes that were in
/// the way went to the Recovery list, and refreshes the tab.
fn restored(tab: TabId, result: Option<Result<Option<RecoveryEntry>, CoreError>>) {
    match result {
        Some(Ok(Some(moved))) => offer_undo(tab, tr_key("recovery.moved"), moved),
        Some(Ok(None)) => toast::show(tr_key("bring_back.done"), None, NOTICE_SECONDS),
        Some(Err(CoreError::RecoveryConflict)) => {
            toast::show(tr_key("recovery.conflict"), None, NOTICE_SECONDS);
        }
        Some(Err(error)) => {
            eprintln!("restoring from the recovery list failed: {error}");
            toast::show(tr_key("recovery.restore_failed"), None, NOTICE_SECONDS);
        }
        None => {}
    }
    with_tab(tab, |t| t.refresh());
}

/// Runs `job` on the project, on its worker, unless the project can only be looked at now
/// (another program started a merge since the last refresh, say): then nothing is touched.
fn when_normal<T>(p: &Project, job: impl FnOnce(&Project) -> T) -> Option<T> {
    (snapshot(p).condition == RepoCondition::Normal).then(|| job(p))
}
