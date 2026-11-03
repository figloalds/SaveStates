//! Load (spec §4): "Carregar este save" in the peek banner, and in the read-only banner of a
//! project left on a bare save (§4.6). What follows a Load is here too: the question about
//! unsaved changes, the toast with Desfazer, "Tentar de novo" for files left over, and the
//! failures.
//!
//! Every Load runs on the tab's worker, which looks again for unsaved changes right before it
//! acts (an AI tool may have written some a moment ago), and ends with a refresh.

use savestates_core::{
    CoreError, LoadError, LoadOutcome, SaveId, continue_from_here, finish_load, load,
};
use savestates_ui_raven::{tr, tr_fmt};

use super::{App, UNDO_SECONDS, with_app, with_tab};
use crate::dialogs::{confirm, inform};
use crate::tabs::{ProjectTab, TabId};
use crate::worker::when_normal;
use crate::{toast, tr_key};

/// The answer "Salvar primeiro" to the question about unsaved changes.
const SAVE_FIRST: i32 = 1;
/// The answer "Tentar de novo" when files were left over.
const RETRY: i32 = 1;

/// A Load asked for: in which tab, to which save, and the timeline to continue when several
/// end at it (where Desfazer goes back to).
struct LoadRequest {
    tab: TabId,
    target: SaveId,
    prefer: Option<String>,
}

/// A Load that is done: the toast's text, and what Desfazer loads.
struct Loaded {
    text: String,
    undo: LoadRequest,
}

impl App {
    /// "Carregar este save" in the peek banner: loads the save peeked at.
    pub(super) fn load_peeked(&self, tab: &ProjectTab) {
        if let Some(target) = tab.load_target() {
            self.load_save(tab, target, None);
        }
    }

    /// Loads `target` in `tab`, continuing `prefer` when several timelines end there. With
    /// unsaved changes (as of the last refresh) it asks to save them first (spec §4.2).
    fn load_save(&self, tab: &ProjectTab, target: SaveId, prefer: Option<String>) {
        let request = LoadRequest {
            tab: tab.id,
            target,
            prefer,
        };
        if tab.has_unsaved() {
            self.ask_save_first(request);
        } else {
            run_load(request);
        }
    }

    /// "Você tem alterações não salvas": "Salvar primeiro" runs the usual Quick Save, and the
    /// Load once the save is made; cancelled or failed, the Load does not happen.
    fn ask_save_first(&self, request: LoadRequest) {
        confirm(
            self.window,
            tr_key("load.unsaved_title"),
            &tr("load.unsaved_body"),
            &[
                (tr_key("load.save_first"), SAVE_FIRST),
                (tr_key("common.cancel"), 0),
            ],
            move |answer| {
                if answer == SAVE_FIRST {
                    with_app(|app| {
                        let tab = request.tab;
                        app.quick_save_then(tab, Box::new(move || run_load(request)));
                    });
                }
            },
        );
    }

    /// A Load came back. `None`: the project could only be looked at by then, and nothing was
    /// touched; the refresh shows why.
    fn loaded(&self, request: LoadRequest, result: Option<Result<LoadOutcome, LoadError>>) {
        let Some(tab) = self.tab(request.tab) else {
            return;
        };
        tab.set_loading(false);
        match result {
            Some(Ok(outcome)) => self.load_done(&tab, &request.target, outcome),
            // Changes appeared since the last refresh: ask now.
            Some(Err(LoadError::HasUnsavedChanges)) => {
                tab.refresh();
                self.ask_save_first(request);
            }
            Some(Err(error)) => {
                tab.refresh();
                self.tell_load_failed(error);
            }
            None => tab.refresh(),
        }
    }

    /// The project moved to `target`: the timeline shows it, then the toast offers Desfazer,
    /// or, when files could not be updated, the question that offers "Tentar de novo".
    fn load_done(&self, tab: &ProjectTab, target: &SaveId, outcome: LoadOutcome) {
        log_outcome(&outcome);
        // Read before the refresh: both saves are loaded on the timeline now.
        let name = tab.save_name(target).unwrap_or_else(|| short(target));
        let LoadOutcome {
            origin,
            set_aside,
            left_over,
            warning,
            ..
        } = outcome;
        let mut text = if set_aside.is_some() {
            tr_fmt("load.done_aside", &[&name])
        } else {
            tr_fmt("load.done", &[&name])
        };
        if warning.is_some() {
            // One toast at a time: the hook's complaint joins the one that offers Desfazer.
            text = format!("{text}\n{}", tr("load.hook_warning"));
        }
        let loaded = Loaded {
            text,
            undo: LoadRequest {
                tab: tab.id,
                target: origin.save.clone(),
                prefer: origin.branch,
            },
        };
        tab.show_loaded();
        if left_over.is_empty() {
            offer_undo(loaded);
        } else {
            self.ask_retry(origin.save, left_over, loaded);
        }
    }

    /// Some files could not be updated (another program has them open): "Tentar de novo"
    /// finishes the Load for them, and asks again while some remain.
    fn ask_retry(&self, origin: SaveId, left_over: Vec<String>, loaded: Loaded) {
        let body = format!("{}\n\n{}", tr("load.left_over_body"), left_over.join("\n"));
        confirm(
            self.window,
            tr_key("load.left_over_title"),
            &body,
            &[(tr_key("load.retry"), RETRY), (tr_key("common.close"), 0)],
            move |answer| {
                if answer == RETRY {
                    run_retry(origin, left_over, loaded);
                }
            },
        );
    }

    /// "Tentar de novo" came back with the files still left over (`left`), or failed, which
    /// leaves the ones of before (`left_before`).
    fn retried(
        &self,
        origin: SaveId,
        loaded: Loaded,
        left: Option<Result<Vec<String>, CoreError>>,
        left_before: Vec<String>,
    ) {
        let Some(tab) = self.tab(loaded.undo.tab) else {
            return;
        };
        tab.refresh();
        match left {
            Some(Ok(left)) if left.is_empty() => offer_undo(loaded),
            Some(Ok(left)) => self.ask_retry(origin, left, loaded),
            Some(Err(error)) => {
                log!("finishing a Load failed: {error}");
                self.ask_retry(origin, left_before, loaded);
            }
            // The project can only be looked at now: the refresh shows why.
            None => {}
        }
    }

    /// Desfazer on the toast: loads the save the project came from, the same way (unsaved
    /// changes included).
    fn undo_load(&self, request: LoadRequest) {
        if let Some(tab) = self.tab(request.tab) {
            self.load_save(&tab, request.target, request.prefer);
        }
    }

    /// "Carregar este save" in the read-only banner of a project on a bare save (spec §4.6):
    /// continues the timeline that ends there, or starts one. No file changes.
    pub(super) fn load_here(&self, tab: &ProjectTab) {
        if tab.is_missing() || !tab.set_loading(true) {
            return;
        }
        let id = tab.id;
        tab.submit(move |p| {
            let result = continue_from_here(p);
            Box::new(move || with_app(|app| app.continued(id, result)))
        });
    }

    fn continued(&self, id: TabId, result: Result<String, LoadError>) {
        let Some(tab) = self.tab(id) else { return };
        tab.set_loading(false);
        tab.refresh();
        match result {
            Ok(branch) => log!("a bare save now continues on {branch}"),
            // Something else is in the way now: the refresh shows it.
            Err(LoadError::NotNormal) => {}
            Err(error) => self.tell_load_failed(error),
        }
    }

    /// Tells the user that a Load did not happen, with git's words under "Details".
    fn tell_load_failed(&self, error: LoadError) {
        match error {
            LoadError::PrefixTaken => inform(
                self.window,
                tr_key("load.failed_title"),
                &tr("load.prefix_taken"),
                None,
            ),
            LoadError::SwitchFailed { details, untouched } => {
                log!("a Load failed: {details}; left alone: {untouched:?}");
                let details = switch_failed_details(&details, &untouched);
                inform(
                    self.window,
                    tr_key("load.failed_title"),
                    &tr("load.failed"),
                    Some(&details),
                );
            }
            LoadError::Core(error) => {
                log!("a Load failed: {error}");
                inform(
                    self.window,
                    tr_key("load.failed_title"),
                    &tr("load.failed"),
                    Some(&error.to_string()),
                );
            }
            // Asked about, or shown by the refresh.
            LoadError::HasUnsavedChanges | LoadError::NotNormal => {}
        }
    }
}

/// Runs the Load on the tab's worker, unless another one is under way there.
fn run_load(request: LoadRequest) {
    with_tab(request.tab, |tab| {
        if tab.is_missing() || !tab.set_loading(true) {
            return;
        }
        tab.submit(move |p| {
            let result = when_normal(p, |p| load(p, &request.target, request.prefer.as_deref()));
            Box::new(move || with_app(|app| app.loaded(request, result)))
        });
    });
}

/// "Tentar de novo": finishes the Load from `origin` on the tab's worker, for the files
/// `left_over`.
fn run_retry(origin: SaveId, left_over: Vec<String>, loaded: Loaded) {
    with_tab(loaded.undo.tab, |tab| {
        tab.submit(move |p| {
            let left = when_normal(p, |p| finish_load(p, &origin));
            Box::new(move || with_app(|app| app.retried(origin, loaded, left, left_over)))
        });
    });
}

/// Offers Desfazer for 10 seconds, under the text that says what was loaded.
fn offer_undo(loaded: Loaded) {
    let undo = loaded.undo;
    let action: Box<dyn FnOnce()> = Box::new(move || with_app(|app| app.undo_load(undo)));
    toast::show_text(
        &loaded.text,
        Some((tr_key("common.undo"), action)),
        UNDO_SECONDS,
    );
}

/// What a failed switch says under "Details": git's own words, then the files it left alone
/// because they changed while the Load ran (they are unsaved changes now).
fn switch_failed_details(details: &str, untouched: &[String]) -> String {
    if untouched.is_empty() {
        details.to_owned()
    } else {
        format!("{details}\n\n{}", untouched.join("\n"))
    }
}

/// For the log: what the Load did besides moving.
fn log_outcome(outcome: &LoadOutcome) {
    if let Some(warning) = &outcome.warning {
        log!("a Load finished with a warning: {warning}");
    }
    if let Some(removed) = &outcome.removed {
        log!("the empty timeline {removed} was removed");
    }
    if !outcome.kept_ignored.is_empty() {
        log!("kept ignored: {}", outcome.kept_ignored.join(", "));
    }
    if outcome.brought_back > 0 {
        log!("{} set-aside snapshot(s) came back", outcome.brought_back);
    }
    if !outcome.left_over.is_empty() {
        log!("left over: {}", outcome.left_over.join(", "));
    }
}

/// A save's name when it is not loaded (it always is, for a Load from the timeline).
fn short(id: &SaveId) -> String {
    id.0.chars().take(7).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_switch_lists_the_files_it_left_alone() {
        assert_eq!(switch_failed_details("error: cannot", &[]), "error: cannot");
        let untouched = ["a.txt".to_owned(), "css/b.css".to_owned()];
        assert_eq!(
            switch_failed_details("error: cannot", &untouched),
            "error: cannot\n\na.txt\ncss/b.css"
        );
    }
}
