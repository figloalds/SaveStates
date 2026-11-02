//! Quick Save (spec §6.7): [`QuickSave`] decides, and this carries out its effects.
//!
//! Git work runs on the tab's worker: the scan, the decisions, who is saving, reading what
//! changed for the AI, and the save. The AI runs on a thread of its own. Every outcome (from
//! those threads, through `task::post`, or from a dialog) comes back tagged with its run, and
//! is dropped if another run started since.
//!
//! The files can change while a dialog is open, so the steps that send or save them look again
//! first: what goes to the AI is scanned again for secrets, and the save is scanned again for
//! anything the user was not asked about.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use savestates_ai::{CancelToken, NamingError, NamingRequest, generate_name, locate_claude};
use savestates_core::settings::NamingMode;
use savestates_core::{
    CoreError, Flag, FlagReason, NamingInput, Project, RepoCondition, SaveError, SaveId, SaveState,
    apply_decisions, commit_all, identity_missing, naming_input, scan, set_identity_if_missing,
    snapshot, timeline_page,
};
use savestates_git::GitError;
use savestates_ui_raven::{task, tr};

use super::{App, NOTICE_SECONDS, with_app, with_tab};
use crate::dialogs::{ai_spinner, identity, inform, name, warnings};
use crate::flows::quick_save::{Effect, QsContext, QsEvent, QuickSave};
use crate::tabs::{ProjectTab, TabId};
use crate::{toast, tr_key};

/// The Quick Save flow (at most one at a time in the app), and what the app keeps for the run
/// under way.
pub(super) struct QuickSaveRun {
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
    /// The flags the user chose an action for in the warnings.
    answered: Vec<Flag>,
    /// The save just made and the one before it, read back for the timeline.
    made: Option<(SaveState, Option<SaveId>)>,
}

impl QuickSaveRun {
    /// No Quick Save under way, the last one being `run`.
    pub(super) fn idle(run: u64) -> QuickSaveRun {
        QuickSaveRun {
            flow: QuickSave::new(),
            run,
            tab: 0,
            then: None,
            claude: None,
            cancel: None,
            answered: Vec::new(),
            made: None,
        }
    }
}

/// What the scan before a Quick Save found.
#[derive(Debug)]
enum Scanned {
    /// The project can only be looked at now (another program started a merge since the last
    /// refresh, say): nothing is saved.
    ReadOnly,
    /// What to tell the flow: the scan, or that there is nothing to save, or that it failed.
    Done(QsEvent),
}

/// What the save job did.
#[derive(Debug)]
enum Saved {
    /// The project can only be looked at now: nothing was saved.
    ReadOnly,
    /// The save, or why it was not made, and the new save read back for the timeline.
    Done(
        Result<SaveId, SaveError>,
        Option<(SaveState, Option<SaveId>)>,
    ),
}

impl App {
    /// Ctrl+S: a Quick Save of the active tab.
    pub(super) fn quick_save_active(&self) {
        let active = self.tabs.borrow().active();
        let tab = active.and_then(|i| self.pages.borrow().get(i).cloned());
        if let Some(tab) = tab {
            self.quick_save_tab(&tab);
        }
    }

    /// "Quick Save" or Ctrl+S on `tab`: nothing in a project that can only be looked at. (With
    /// nothing to save, the scan says so.)
    pub(super) fn quick_save_tab(&self, tab: &ProjectTab) {
        if tab.can_save() {
            self.start_quick_save(tab.id, None);
        }
    }

    /// Starts a Quick Save of the tab `tab`; with `prefill`, the name dialog starts with it
    /// instead of asking the AI. Nothing happens while another Quick Save is under way.
    pub fn start_quick_save(&self, tab: TabId, prefill: Option<String>) {
        self.begin_quick_save(tab, prefill, None);
    }

    /// Quick Saves the project of the tab `tab`, then runs `continuation` once the save is made
    /// ("Quick Save first", before bringing everything back over unsaved changes). When the
    /// Quick Save ends any other way (cancelled, failed, nothing to save), or another one is
    /// under way, `continuation` is dropped without running.
    pub fn quick_save_then(&self, tab: TabId, continuation: Box<dyn FnOnce()>) {
        self.begin_quick_save(tab, None, Some(continuation));
    }

    /// The tab `tab` closed: a Quick Save of its project ends there.
    pub(super) fn quick_save_tab_closed(&self, tab: TabId) {
        let saving = {
            let quick_save = self.quick_save.borrow();
            quick_save.flow.is_busy() && quick_save.tab == tab
        };
        if saving {
            self.abandon_quick_save();
        }
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
            Effect::ApplyDecisions(decisions) => {
                self.quick_save.borrow_mut().answered =
                    decisions.iter().map(|(flag, _)| flag.clone()).collect();
                tab.submit(move |p| {
                    let ev = match apply_decisions(p, &decisions) {
                        Ok(decision) => QsEvent::DecisionsApplied(decision),
                        Err(error) => QsEvent::SaveDone(Err(save_error(error))),
                    };
                    back_to(run, ev)
                });
            }
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
        {
            let mut quick_save = self.quick_save.borrow_mut();
            if quick_save.run == run {
                quick_save.claude = claude;
            }
        }
        match scanned {
            Scanned::Done(ev) => self.quick_save_event(run, ev),
            Scanned::ReadOnly => self.turned_read_only(run),
        }
    }

    /// The project of the Quick Save `run` can no longer be saved to: the run ends, and the
    /// refresh shows why and turns Quick Save off.
    fn turned_read_only(&self, run: u64) {
        let tab = {
            let quick_save = self.quick_save.borrow();
            (quick_save.run == run).then_some(quick_save.tab)
        };
        if let Some(tab) = tab {
            self.abandon_quick_save();
            with_tab(tab, |tab| tab.refresh());
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

    /// Reads what the save would record on the tab's worker (minus `excluded`, and minus any
    /// secret found by looking again), then asks the AI for a name on a thread of its own.
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
        let (model, timeout, large_file_mb) = {
            let settings = self.settings.borrow();
            let timeout = Duration::from_secs(settings.ai_timeout_secs);
            (settings.ai_model.clone(), timeout, settings.large_file_mb)
        };
        let lang = self.lang;
        tab.submit(move |p| match ai_input(p, excluded, large_file_mb) {
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
                // Nothing is sent.
                let detail = format!("the changes could not be read: {error}");
                let failed = NamingError::Failed { code: None, detail };
                back_to(run, QsEvent::AiDone(Err(failed)))
            }
        });
    }

    /// Saves everything on the tab's worker, once a last look finds nothing new to ask about,
    /// and reads the new save back for the timeline.
    fn run_commit(&self, run: u64, tab: &ProjectTab, name: String, checked: Vec<String>) {
        let large_file_mb = self.settings.borrow().large_file_mb;
        let answered = self.quick_save.borrow().answered.clone();
        tab.submit(move |p| {
            let saved = save_job(p, &name, &checked, &answered, large_file_mb);
            Box::new(move || with_app(|app| app.saved(run, saved)))
        });
    }

    fn saved(&self, run: u64, saved: Saved) {
        match saved {
            Saved::ReadOnly => self.turned_read_only(run),
            Saved::Done(result, made) => {
                {
                    let mut quick_save = self.quick_save.borrow_mut();
                    if quick_save.run != run {
                        return;
                    }
                    quick_save.made = made;
                }
                self.quick_save_event(run, QsEvent::SaveDone(result));
            }
        }
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

/// What the AI is given to name the save, on the project's thread: what the save would record,
/// minus `excluded` and minus every file a scan made right now finds a secret in. A file can
/// change while a dialog is open, after the scan the user saw. (Whatever still looks like a
/// token in what is sent is redacted by `naming_input`.)
fn ai_input(
    p: &Project,
    mut excluded: Vec<String>,
    large_file_mb: u64,
) -> Result<NamingInput, CoreError> {
    let snap = snapshot(p);
    for flag in scan(p, &snap.changes, large_file_mb)? {
        let secret = matches!(
            flag.reason,
            FlagReason::SecretName | FlagReason::SecretContent { .. }
        );
        if secret && !excluded.contains(&flag.path) {
            excluded.push(flag.path);
        }
    }
    naming_input(p, &excluded)
}

/// The save, on the project's thread. A file can change while a dialog is open, so the
/// project is looked at once more first: nothing is saved when it can only be looked at now
/// (another program started a merge, which a save would complete), nor when the scan finds
/// something the user was not asked about (reported as [`SaveError::ChangedDuringSave`], so
/// the next Quick Save shows it). `commit_all` guards the moment that is left.
fn save_job(
    p: &Project,
    name: &str,
    checked: &[String],
    answered: &[Flag],
    large_file_mb: u64,
) -> Saved {
    let snap = snapshot(p);
    if snap.condition != RepoCondition::Normal {
        return Saved::ReadOnly;
    }
    let result = scan(p, &snap.changes, large_file_mb)
        .map_err(save_error)
        .and_then(|found| {
            let paths = unanswered(answered, &found);
            if paths.is_empty() {
                commit_all(p, name, checked, large_file_mb)
            } else {
                Err(SaveError::ChangedDuringSave { paths })
            }
        });
    let made = result.as_ref().ok().and_then(|id| new_save(p, id));
    Saved::Done(result, made)
}

/// The paths of the flags in `found` that the user did not answer: no answered flag has the
/// same path and the same kind of reason. (A secret that moved to another line, or a large
/// file that grew, is the same warning.)
fn unanswered(answered: &[Flag], found: &[Flag]) -> Vec<String> {
    let same = |a: &Flag, b: &Flag| {
        a.path == b.path && std::mem::discriminant(&a.reason) == std::mem::discriminant(&b.reason)
    };
    found
        .iter()
        .filter(|flag| !answered.iter().any(|answered| same(answered, flag)))
        .map(|flag| flag.path.clone())
        .collect()
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

#[cfg(test)]
mod tests {
    // testkit is safe in this crate's unit tests: the app is a binary that nothing depends on,
    // so its tests and testkit share the one build of savestates-git.
    use super::*;
    use savestates_core::{OpenOutcome, open_project};
    use testkit::TestRepo;

    /// A token-shaped test string, never a real key.
    const ANT_KEY: &str = "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789";
    const LARGE_MB: u64 = 25;

    fn flag(path: &str, reason: FlagReason) -> Flag {
        Flag {
            path: path.to_owned(),
            reason,
            already_saved: false,
            in_older_saves: false,
        }
    }

    fn open(repo: &TestRepo) -> Project {
        match open_project(&repo.exe(), repo.path()).unwrap() {
            OpenOutcome::Opened(project) => project,
            other => panic!("expected a project, got {other:?}"),
        }
    }

    /// The scan the user is shown: its flags and the paths it checked.
    fn user_scan(p: &Project) -> (Vec<Flag>, Vec<String>) {
        match scan_for_save(p, LARGE_MB) {
            Scanned::Done(QsEvent::ScanDone { flags, checked, .. }) => (flags, checked),
            other => panic!("expected a scan, got {other:?}"),
        }
    }

    fn head(repo: &TestRepo) -> String {
        repo.git(&["rev-parse", "HEAD"]).trim().to_owned()
    }

    #[test]
    fn only_the_flags_nobody_answered_are_new() {
        let answered = [
            flag("src/api.js", FlagReason::SecretContent { line: 3 }),
            flag("node_modules", FlagReason::JunkFolder),
            flag("video.mp4", FlagReason::Large { bytes: 30 << 20 }),
        ];
        let found = [
            // The same flags: the key moved to another line, the video grew.
            flag("src/api.js", FlagReason::SecretContent { line: 7 }),
            flag("video.mp4", FlagReason::Large { bytes: 31 << 20 }),
            flag("node_modules", FlagReason::JunkFolder),
            // New ones.
            flag("config.js", FlagReason::SecretContent { line: 1 }),
            flag(".env", FlagReason::SecretName),
        ];
        assert_eq!(unanswered(&answered, &found), ["config.js", ".env"]);
        // The same path flagged for another reason was not answered.
        let other_reason = [flag("video.mp4", FlagReason::SecretContent { line: 2 })];
        assert_eq!(unanswered(&answered, &other_reason), ["video.mp4"]);
        assert!(unanswered(&answered, &[]).is_empty());
        assert_eq!(
            unanswered(&[], &[flag(".env", FlagReason::SecretName)]),
            [".env"]
        );
    }

    #[test]
    fn a_key_pasted_into_a_checked_file_after_the_scan_is_not_saved() {
        let repo = TestRepo::new();
        repo.write("src/api.js", "fetch(url);\n");
        let before = repo.commit_all("first");
        repo.write("src/api.js", "fetch(url);\nconsole.log(1);\n");
        let p = open(&repo);
        let (flags, checked) = user_scan(&p);
        assert!(flags.is_empty(), "{flags:?}");
        assert_eq!(checked, ["src/api.js"]);

        // While the name dialog is open, a key lands in the file the user was shown.
        repo.write(
            "src/api.js",
            format!("fetch(url, \"{ANT_KEY}\");\nconsole.log(1);\n"),
        );

        match save_job(&p, "Logs", &checked, &flags, LARGE_MB) {
            Saved::Done(Err(SaveError::ChangedDuringSave { paths }), None) => {
                assert_eq!(paths, ["src/api.js"]);
            }
            other => panic!("expected changed during save, got {other:?}"),
        }
        assert_eq!(head(&repo), before, "nothing was saved");
    }

    #[test]
    fn what_the_user_answered_is_saved() {
        let repo = TestRepo::new();
        repo.write("src/api.js", "fetch(url);\n");
        let before = repo.commit_all("first");
        repo.write("src/api.js", format!("fetch(url, \"{ANT_KEY}\");\n"));
        let p = open(&repo);
        let (flags, checked) = user_scan(&p);
        let found: Vec<_> = flags.iter().map(|f| (f.path.as_str(), &f.reason)).collect();
        assert_eq!(
            found,
            [("src/api.js", &FlagReason::SecretContent { line: 1 })]
        );
        // "Save anyway"; meanwhile the key moves down a line, which is still the same warning.
        repo.write(
            "src/api.js",
            format!("// api\nfetch(url, \"{ANT_KEY}\");\n"),
        );

        match save_job(&p, "Com a chave", &checked, &flags, LARGE_MB) {
            Saved::Done(Ok(id), Some((save, parent))) => {
                assert_eq!(save.id, id);
                assert_eq!(save.name, "Com a chave");
                assert_eq!(parent, Some(SaveId(before)));
                assert_eq!(head(&repo), id.0);
            }
            other => panic!("expected a save, got {other:?}"),
        }
    }

    #[test]
    fn a_project_another_program_started_merging_is_not_saved() {
        let repo = TestRepo::new();
        repo.write("a.txt", "one\n");
        let before = repo.commit_all("first");
        repo.write("a.txt", "two\n");
        let p = open(&repo);
        let (flags, checked) = user_scan(&p);

        // During the name dialog, another program starts a merge.
        repo.write(".git/MERGE_HEAD", format!("{before}\n"));

        assert!(
            matches!(
                save_job(&p, "x", &checked, &flags, LARGE_MB),
                Saved::ReadOnly
            ),
            "a save now would complete the other program's merge"
        );
        assert_eq!(head(&repo), before);
    }

    #[test]
    fn secrets_written_after_the_scan_are_kept_from_the_ai() {
        let repo = TestRepo::new();
        repo.write("index.html", "<h1>Oi</h1>\n");
        repo.commit_all("first");
        repo.write("index.html", "<h1>Olá</h1>\n");
        let p = open(&repo);
        let (flags, _) = user_scan(&p);
        assert!(flags.is_empty(), "{flags:?}");

        // While the warnings or the identity dialog is open: a secrets file whose text looks
        // like nothing in particular, and a key in a new file.
        repo.write(".env", "DB_PASSWORD=hunter2\n");
        repo.write("config.js", format!("export const key = \"{ANT_KEY}\";\n"));

        let input = ai_input(&p, Vec::new(), LARGE_MB).unwrap();
        assert!(input.stat.contains("index.html"), "{}", input.stat);
        assert!(!input.stat.contains(".env"), "{}", input.stat);
        assert!(!input.patch.contains("hunter2"), "{}", input.patch);
        assert!(!input.stat.contains("config.js"), "{}", input.stat);
        assert!(!input.patch.contains(ANT_KEY), "{}", input.patch);
    }
}
