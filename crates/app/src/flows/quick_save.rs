//! Quick Save (spec §6.7), step by step: the safety scan, the warnings (when something was
//! flagged) and the user's decisions, who is saving (asked once), the name (typed, or suggested
//! by the AI), then the save itself.
//!
//! [`QuickSave`] only decides: each [`QsEvent`] gives the [`Effect`]s the app carries out (run
//! the scan on the project's thread, show a dialog...), whose outcome comes back as the next
//! event. It touches neither git nor the screen, so every path is unit-tested.

use savestates_ai::NamingError;
use savestates_core::settings::NamingMode;
use savestates_core::{Decision, Flag, FlagAction, FlagReason, SaveError, SaveId};
use savestates_git::GitError;

use crate::tr_key;

/// What the flow needs to know about the app when it decides.
#[derive(Clone, Copy, Debug)]
pub struct QsContext {
    /// How saves get their name (the setting).
    pub naming: NamingMode,
    /// The Claude CLI was found.
    pub claude_available: bool,
}

/// Why the AI gave no name: a one-line note in the name dialog (`ai.note.*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiNote {
    NotInstalled,
    NotLoggedIn,
    Timeout,
    Failed,
    Empty,
}

/// What happened: the user's answers, and the outcome of the work the effects asked for.
#[derive(Debug)]
pub enum QsEvent {
    /// The user asked for a Quick Save. With a `prefill`, the name dialog shows it (and the AI
    /// is not asked).
    Start {
        prefill: Option<String>,
    },
    /// The safety scan ran. `checked` lists every path the user is shown the save of: the
    /// unsaved changes the scan looked at.
    ScanDone {
        flags: Vec<Flag>,
        identity_missing: bool,
        checked: Vec<String>,
    },
    /// The user chose an action for each flag.
    WarningsAnswered(Vec<(Flag, FlagAction)>),
    WarningsCancelled,
    DecisionsApplied(Decision),
    /// Who is saving was written.
    IdentityGiven,
    IdentityCancelled,
    AiDone(Result<String, NamingError>),
    /// "Cancel" while the AI thinks.
    AiCancelRequested,
    NameConfirmed(String),
    NameCancelled,
    /// The save ended. It also ends the flow at any other step that waits for the project's
    /// thread (the scan, applying the decisions, writing who is saving): a failure there comes
    /// as a failed save.
    SaveDone(Result<SaveId, SaveError>),
}

/// What the app is to do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Scan the unsaved changes on the project's thread, and say whether who is saving is
    /// unknown (then [`QsEvent::ScanDone`]).
    RunScan,
    ShowWarnings(Vec<Flag>),
    /// Carry out the decisions on the project's thread (then [`QsEvent::DecisionsApplied`]).
    ApplyDecisions(Vec<(Flag, FlagAction)>),
    /// Ask who is saving, and write it (then [`QsEvent::IdentityGiven`]).
    AskIdentity,
    ShowAiSpinner,
    HideAiSpinner,
    /// Ask the AI for a name, leaving out what is under `excluded` (then [`QsEvent::AiDone`]).
    RunAi {
        excluded: Vec<String>,
    },
    /// Stop the AI.
    CancelAi,
    ShowNameDialog {
        prefill: String,
        note: Option<AiNote>,
    },
    /// Save everything under `name` (then [`QsEvent::SaveDone`]).
    RunCommit {
        name: String,
        checked: Vec<String>,
    },
    /// Show this file among the unsaved changes, so the user can fix it.
    SelectFile(String),
    /// A commit hook stopped the save; what it printed.
    ShowHookRejected(String),
    /// Files that need a decision appeared while saving; the save was not made.
    ShowChangedDuringSave(Vec<String>),
    /// Another program holds the project's files.
    ShowLocked,
    /// Any other failure, with its technical details.
    ShowError(String),
    /// A short message: the translation key.
    Toast(&'static str),
    /// The new save grows into the timeline.
    AnimateNewStation(SaveId),
    /// Read the project's state again.
    Refresh,
}

/// The Quick Save flow. At most one runs at a time.
#[derive(Debug, Default)]
pub struct QuickSave {
    state: State,
}

/// What the steps after the warnings carry along.
#[derive(Debug)]
struct Plan {
    /// The name the dialog starts with, instead of asking the AI.
    prefill: Option<String>,
    /// What `commit_all` may save without asking: the unsaved changes the user was shown, and
    /// the junk folders they chose to save anyway.
    checked: Vec<String>,
    /// What the AI must not see: possible secrets, and what is left out of the save.
    excluded: Vec<String>,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Idle,
    Scanning {
        prefill: Option<String>,
    },
    Warnings {
        plan: Plan,
        identity_missing: bool,
    },
    Applying {
        plan: Plan,
        identity_missing: bool,
    },
    Identity {
        plan: Plan,
    },
    Thinking {
        checked: Vec<String>,
    },
    Naming {
        checked: Vec<String>,
    },
    Committing,
}

impl QuickSave {
    pub fn new() -> Self {
        Self::default()
    }

    /// A flow is under way: [`QsEvent::Start`] does nothing.
    pub fn is_busy(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// Takes `ev` and says what to do next. An event that does not fit the current step (a late
    /// answer from the AI, a second Start) changes nothing and gives nothing to do.
    pub fn step(&mut self, ev: QsEvent, ctx: &QsContext) -> Vec<Effect> {
        let (state, effects) = advance(std::mem::take(&mut self.state), ev, ctx);
        self.state = state;
        effects
    }
}

fn advance(state: State, ev: QsEvent, ctx: &QsContext) -> (State, Vec<Effect>) {
    use QsEvent as E;
    match (state, ev) {
        (State::Idle, E::Start { prefill }) => (State::Scanning { prefill }, vec![Effect::RunScan]),
        (
            State::Scanning { prefill },
            E::ScanDone {
                flags,
                identity_missing,
                checked,
            },
        ) => {
            let plan = Plan {
                prefill,
                checked,
                excluded: Vec::new(),
            };
            if flags.is_empty() {
                after_warnings(plan, identity_missing, ctx)
            } else {
                let state = State::Warnings {
                    plan,
                    identity_missing,
                };
                (state, vec![Effect::ShowWarnings(flags)])
            }
        }
        (
            State::Warnings {
                mut plan,
                identity_missing,
            },
            E::WarningsAnswered(decisions),
        ) => {
            for (flag, action) in &decisions {
                let secret = matches!(
                    flag.reason,
                    FlagReason::SecretName | FlagReason::SecretContent { .. }
                );
                let left_out = matches!(action, FlagAction::LeaveOut | FlagAction::StopSaving);
                if secret || left_out {
                    push_new(&mut plan.excluded, &flag.path);
                }
                if flag.reason == FlagReason::JunkFolder && *action == FlagAction::SaveAnyway {
                    push_new(&mut plan.checked, &flag.path);
                }
            }
            let state = State::Applying {
                plan,
                identity_missing,
            };
            (state, vec![Effect::ApplyDecisions(decisions)])
        }
        (State::Warnings { .. }, E::WarningsCancelled) => (State::Idle, Vec::new()),
        (State::Applying { .. }, E::DecisionsApplied(Decision::CancelAndFix { path })) => {
            (State::Idle, vec![Effect::SelectFile(path)])
        }
        (
            State::Applying {
                mut plan,
                identity_missing,
            },
            E::DecisionsApplied(Decision::Proceed { excluded }),
        ) => {
            for path in &excluded {
                push_new(&mut plan.excluded, path);
            }
            after_warnings(plan, identity_missing, ctx)
        }
        (State::Identity { plan }, E::IdentityGiven) => ask_for_name(plan, ctx),
        (State::Identity { .. }, E::IdentityCancelled) => (State::Idle, Vec::new()),
        (State::Thinking { checked }, E::AiDone(result)) => {
            let (prefill, note) = match result {
                Ok(name) => (name, None),
                Err(error) => (String::new(), ai_note(&error)),
            };
            let effects = vec![
                Effect::HideAiSpinner,
                Effect::ShowNameDialog { prefill, note },
            ];
            (State::Naming { checked }, effects)
        }
        (State::Thinking { checked }, E::AiCancelRequested) => {
            let effects = vec![
                Effect::CancelAi,
                Effect::HideAiSpinner,
                Effect::ShowNameDialog {
                    prefill: String::new(),
                    note: None,
                },
            ];
            (State::Naming { checked }, effects)
        }
        (State::Naming { checked }, E::NameConfirmed(name)) => {
            (State::Committing, vec![Effect::RunCommit { name, checked }])
        }
        (State::Naming { .. }, E::NameCancelled) => (State::Idle, Vec::new()),
        (
            State::Scanning { .. }
            | State::Applying { .. }
            | State::Identity { .. }
            | State::Committing,
            E::SaveDone(result),
        ) => (State::Idle, finished(result)),
        (state, _) => (state, Vec::new()),
    }
}

/// The warnings are dealt with: who is saving, if unknown, then the name.
fn after_warnings(plan: Plan, identity_missing: bool, ctx: &QsContext) -> (State, Vec<Effect>) {
    if identity_missing {
        (State::Identity { plan }, vec![Effect::AskIdentity])
    } else {
        ask_for_name(plan, ctx)
    }
}

/// The name: the prefilled one, a typed one, or the AI's suggestion when the setting says so
/// and Claude is there.
fn ask_for_name(plan: Plan, ctx: &QsContext) -> (State, Vec<Effect>) {
    let Plan {
        prefill,
        checked,
        excluded,
    } = plan;
    let dialog = |prefill: String, note| {
        (
            State::Naming {
                checked: checked.clone(),
            },
            vec![Effect::ShowNameDialog { prefill, note }],
        )
    };
    match (prefill, ctx.naming) {
        (Some(prefill), _) => dialog(prefill, None),
        (None, NamingMode::AskMe) => dialog(String::new(), None),
        (None, NamingMode::Ai) if !ctx.claude_available => {
            dialog(String::new(), Some(AiNote::NotInstalled))
        }
        (None, NamingMode::Ai) => (
            State::Thinking { checked },
            vec![Effect::ShowAiSpinner, Effect::RunAi { excluded }],
        ),
    }
}

/// The note for an AI failure; none when the user cancelled.
fn ai_note(error: &NamingError) -> Option<AiNote> {
    match error {
        NamingError::NotInstalled => Some(AiNote::NotInstalled),
        NamingError::NotLoggedIn => Some(AiNote::NotLoggedIn),
        NamingError::Timeout => Some(AiNote::Timeout),
        NamingError::Cancelled => None,
        NamingError::Failed { .. } => Some(AiNote::Failed),
        NamingError::EmptyOutput => Some(AiNote::Empty),
    }
}

/// What the end of a save shows.
fn finished(result: Result<SaveId, SaveError>) -> Vec<Effect> {
    match result {
        Ok(id) => vec![Effect::AnimateNewStation(id), Effect::Refresh],
        Err(SaveError::NothingToSave) => {
            vec![Effect::Toast(tr_key("save.nothing_new")), Effect::Refresh]
        }
        Err(SaveError::HookRejected { output }) => vec![Effect::ShowHookRejected(output)],
        Err(SaveError::ChangedDuringSave { paths }) => {
            vec![Effect::ShowChangedDuringSave(paths), Effect::Refresh]
        }
        Err(SaveError::Git(GitError::Locked)) => vec![Effect::ShowLocked],
        Err(other) => vec![Effect::ShowError(other.to_string())],
    }
}

/// Adds `path` to `paths` unless it is there already.
fn push_new(paths: &mut Vec<String>, path: &str) {
    if !paths.iter().any(|p| p == path) {
        paths.push(path.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Effect::*;
    use QsEvent::*;
    use savestates_ai::NamingError;
    use savestates_core::settings::NamingMode;
    use savestates_core::{Decision, Flag, FlagAction, FlagReason, SaveError, SaveId};
    use savestates_git::GitError;

    fn ask_me() -> QsContext {
        QsContext {
            naming: NamingMode::AskMe,
            claude_available: true,
        }
    }

    fn ai() -> QsContext {
        QsContext {
            naming: NamingMode::Ai,
            claude_available: true,
        }
    }

    fn flag(path: &str, reason: FlagReason) -> Flag {
        Flag {
            path: path.to_owned(),
            reason,
            already_saved: false,
            in_older_saves: false,
        }
    }

    fn checked() -> Vec<String> {
        vec!["index.html".to_owned(), "src/api.js".to_owned()]
    }

    fn scanned(flags: Vec<Flag>, identity_missing: bool) -> QsEvent {
        ScanDone {
            flags,
            identity_missing,
            checked: checked(),
        }
    }

    fn id() -> SaveId {
        SaveId("a".repeat(40))
    }

    fn name_dialog(prefill: &str, note: Option<AiNote>) -> Effect {
        ShowNameDialog {
            prefill: prefill.to_owned(),
            note,
        }
    }

    /// A flow in Ask mode, up to the commit: returns the `checked` it was given.
    fn committing(qs: &mut QuickSave) -> Vec<String> {
        let ctx = ask_me();
        assert_eq!(qs.step(Start { prefill: None }, &ctx), [RunScan]);
        assert_eq!(
            qs.step(scanned(vec![], false), &ctx),
            [name_dialog("", None)]
        );
        let effects = qs.step(NameConfirmed("x".to_owned()), &ctx);
        assert_eq!(
            effects,
            [RunCommit {
                name: "x".to_owned(),
                checked: checked()
            }]
        );
        checked()
    }

    #[test]
    fn ask_me_happy_path() {
        let ctx = ask_me();
        let mut qs = QuickSave::new();
        assert!(!qs.is_busy());
        assert_eq!(qs.step(Start { prefill: None }, &ctx), [RunScan]);
        assert!(qs.is_busy());
        assert_eq!(
            qs.step(scanned(vec![], false), &ctx),
            [name_dialog("", None)]
        );
        assert_eq!(
            qs.step(NameConfirmed("x".to_owned()), &ctx),
            [RunCommit {
                name: "x".to_owned(),
                checked: checked()
            }]
        );
        assert_eq!(
            qs.step(SaveDone(Ok(id())), &ctx),
            [AnimateNewStation(id()), Refresh]
        );
        assert!(!qs.is_busy());
    }

    #[test]
    fn warnings_then_fix() {
        let ctx = ask_me();
        let mut qs = QuickSave::new();
        let flags = vec![flag("src/api.js", FlagReason::SecretContent { line: 3 })];
        qs.step(Start { prefill: None }, &ctx);
        assert_eq!(
            qs.step(scanned(flags.clone(), false), &ctx),
            [ShowWarnings(flags.clone())]
        );
        let decisions = vec![(flags[0].clone(), FlagAction::CancelAndFix)];
        assert_eq!(
            qs.step(WarningsAnswered(decisions.clone()), &ctx),
            [ApplyDecisions(decisions)]
        );
        let fix = Decision::CancelAndFix {
            path: "src/api.js".to_owned(),
        };
        assert_eq!(
            qs.step(DecisionsApplied(fix), &ctx),
            [SelectFile("src/api.js".to_owned())]
        );
        assert!(!qs.is_busy());
    }

    #[test]
    fn warnings_cancelled_ends_the_flow() {
        let ctx = ask_me();
        let mut qs = QuickSave::new();
        qs.step(Start { prefill: None }, &ctx);
        qs.step(
            scanned(vec![flag(".env", FlagReason::SecretName)], false),
            &ctx,
        );
        assert_eq!(qs.step(WarningsCancelled, &ctx), []);
        assert!(!qs.is_busy());
    }

    #[test]
    fn identity_before_name() {
        let ctx = ask_me();
        let mut qs = QuickSave::new();
        qs.step(Start { prefill: None }, &ctx);
        assert_eq!(qs.step(scanned(vec![], true), &ctx), [AskIdentity]);
        assert_eq!(qs.step(IdentityGiven, &ctx), [name_dialog("", None)]);

        let mut qs = QuickSave::new();
        qs.step(Start { prefill: None }, &ctx);
        assert_eq!(qs.step(scanned(vec![], true), &ctx), [AskIdentity]);
        assert_eq!(qs.step(IdentityCancelled, &ctx), []);
        assert!(!qs.is_busy());
    }

    #[test]
    fn identity_comes_after_the_warnings() {
        let ctx = ask_me();
        let mut qs = QuickSave::new();
        let env = flag(".env", FlagReason::SecretName);
        qs.step(Start { prefill: None }, &ctx);
        assert_eq!(
            qs.step(scanned(vec![env.clone()], true), &ctx),
            [ShowWarnings(vec![env.clone()])]
        );
        qs.step(WarningsAnswered(vec![(env, FlagAction::LeaveOut)]), &ctx);
        let applied = Decision::Proceed {
            excluded: vec![".env".to_owned()],
        };
        assert_eq!(qs.step(DecisionsApplied(applied), &ctx), [AskIdentity]);
    }

    #[test]
    fn ai_success() {
        let ctx = ai();
        let mut qs = QuickSave::new();
        qs.step(Start { prefill: None }, &ctx);
        assert_eq!(
            qs.step(scanned(vec![], false), &ctx),
            [ShowAiSpinner, RunAi { excluded: vec![] }]
        );
        assert_eq!(
            qs.step(AiDone(Ok("n".to_owned())), &ctx),
            [HideAiSpinner, name_dialog("n", None)]
        );
    }

    #[test]
    fn ai_timeout_note() {
        let ctx = ai();
        let mut qs = QuickSave::new();
        qs.step(Start { prefill: None }, &ctx);
        qs.step(scanned(vec![], false), &ctx);
        assert_eq!(
            qs.step(AiDone(Err(NamingError::Timeout)), &ctx),
            [HideAiSpinner, name_dialog("", Some(AiNote::Timeout))]
        );
    }

    #[test]
    fn every_ai_failure_has_its_note_and_cancelling_has_none() {
        let failures = [
            (NamingError::NotInstalled, Some(AiNote::NotInstalled)),
            (NamingError::NotLoggedIn, Some(AiNote::NotLoggedIn)),
            (NamingError::Timeout, Some(AiNote::Timeout)),
            (NamingError::Cancelled, None),
            (
                NamingError::Failed {
                    code: Some(1),
                    detail: "boom".to_owned(),
                },
                Some(AiNote::Failed),
            ),
            (NamingError::EmptyOutput, Some(AiNote::Empty)),
        ];
        for (error, note) in failures {
            let ctx = ai();
            let mut qs = QuickSave::new();
            qs.step(Start { prefill: None }, &ctx);
            qs.step(scanned(vec![], false), &ctx);
            assert_eq!(
                qs.step(AiDone(Err(error)), &ctx),
                [HideAiSpinner, name_dialog("", note)]
            );
        }
    }

    #[test]
    fn ai_cancel_gives_the_empty_dialog_and_a_late_answer_is_ignored() {
        let ctx = ai();
        let mut qs = QuickSave::new();
        qs.step(Start { prefill: None }, &ctx);
        qs.step(scanned(vec![], false), &ctx);
        assert_eq!(
            qs.step(AiCancelRequested, &ctx),
            [CancelAi, HideAiSpinner, name_dialog("", None)]
        );
        assert_eq!(qs.step(AiDone(Err(NamingError::Cancelled)), &ctx), []);
        assert_eq!(qs.step(AiDone(Ok("late".to_owned())), &ctx), []);
        assert_eq!(
            qs.step(NameConfirmed("mine".to_owned()), &ctx),
            [RunCommit {
                name: "mine".to_owned(),
                checked: checked()
            }]
        );
    }

    #[test]
    fn ai_unavailable() {
        let ctx = QsContext {
            naming: NamingMode::Ai,
            claude_available: false,
        };
        let mut qs = QuickSave::new();
        qs.step(Start { prefill: None }, &ctx);
        assert_eq!(
            qs.step(scanned(vec![], false), &ctx),
            [name_dialog("", Some(AiNote::NotInstalled))]
        );
    }

    #[test]
    fn ai_excludes_secrets() {
        let ctx = ai();
        let mut qs = QuickSave::new();
        let api = flag("src/api.js", FlagReason::SecretContent { line: 3 });
        let env = flag(".env", FlagReason::SecretName);
        qs.step(Start { prefill: None }, &ctx);
        qs.step(scanned(vec![api.clone(), env.clone()], false), &ctx);
        qs.step(
            WarningsAnswered(vec![
                (api, FlagAction::SaveAnyway),
                (env, FlagAction::LeaveOut),
            ]),
            &ctx,
        );
        let applied = Decision::Proceed {
            excluded: vec![".env".to_owned()],
        };
        let effects = qs.step(DecisionsApplied(applied), &ctx);
        let [ShowAiSpinner, RunAi { excluded }] = effects.as_slice() else {
            panic!("expected the spinner and the AI, got {effects:?}");
        };
        let mut excluded = excluded.clone();
        excluded.sort();
        assert_eq!(excluded, [".env", "src/api.js"]);
    }

    #[test]
    fn a_junk_folder_saved_anyway_is_checked_and_what_is_left_out_is_not() {
        let ctx = ask_me();
        let mut qs = QuickSave::new();
        let junk = flag("node_modules", FlagReason::JunkFolder);
        let large = flag("video.mp4", FlagReason::Large { bytes: 30 << 20 });
        qs.step(Start { prefill: None }, &ctx);
        qs.step(scanned(vec![junk.clone(), large.clone()], false), &ctx);
        qs.step(
            WarningsAnswered(vec![
                (junk, FlagAction::SaveAnyway),
                (large, FlagAction::LeaveOut),
            ]),
            &ctx,
        );
        let applied = Decision::Proceed {
            excluded: vec!["video.mp4".to_owned()],
        };
        assert_eq!(
            qs.step(DecisionsApplied(applied), &ctx),
            [name_dialog("", None)]
        );
        let mut expected = checked();
        expected.push("node_modules".to_owned());
        assert_eq!(
            qs.step(NameConfirmed("x".to_owned()), &ctx),
            [RunCommit {
                name: "x".to_owned(),
                checked: expected
            }]
        );
    }

    #[test]
    fn prefill_skips_ai() {
        let ctx = ai();
        let mut qs = QuickSave::new();
        let prefill = Some("Começo do projeto".to_owned());
        assert_eq!(qs.step(Start { prefill }, &ctx), [RunScan]);
        assert_eq!(
            qs.step(scanned(vec![], false), &ctx),
            [name_dialog("Começo do projeto", None)]
        );
    }

    #[test]
    fn name_cancelled_ends_the_flow() {
        let ctx = ask_me();
        let mut qs = QuickSave::new();
        qs.step(Start { prefill: None }, &ctx);
        qs.step(scanned(vec![], false), &ctx);
        assert_eq!(qs.step(NameCancelled, &ctx), []);
        assert!(!qs.is_busy());
    }

    #[test]
    fn hook_rejected() {
        let mut qs = QuickSave::new();
        committing(&mut qs);
        let rejected = SaveError::HookRejected {
            output: "lint failed".to_owned(),
        };
        assert_eq!(
            qs.step(SaveDone(Err(rejected)), &ask_me()),
            [ShowHookRejected("lint failed".to_owned())]
        );
        assert!(!qs.is_busy());
    }

    #[test]
    fn changed_during_save() {
        let mut qs = QuickSave::new();
        committing(&mut qs);
        let paths = vec![".env.local".to_owned(), "dist".to_owned()];
        let changed = SaveError::ChangedDuringSave {
            paths: paths.clone(),
        };
        assert_eq!(
            qs.step(SaveDone(Err(changed)), &ask_me()),
            [ShowChangedDuringSave(paths), Refresh]
        );
        assert!(!qs.is_busy());
    }

    #[test]
    fn locked() {
        let mut qs = QuickSave::new();
        committing(&mut qs);
        assert_eq!(
            qs.step(SaveDone(Err(SaveError::Git(GitError::Locked))), &ask_me()),
            [ShowLocked]
        );
        assert!(!qs.is_busy());
    }

    #[test]
    fn nothing() {
        let mut qs = QuickSave::new();
        committing(&mut qs);
        assert_eq!(
            qs.step(SaveDone(Err(SaveError::NothingToSave)), &ask_me()),
            [Toast("save.nothing_new"), Refresh]
        );
        assert!(!qs.is_busy());
    }

    #[test]
    fn any_other_failure_is_shown_with_its_details() {
        let mut qs = QuickSave::new();
        committing(&mut qs);
        let failed = GitError::Failed {
            args: "commit".to_owned(),
            code: Some(128),
            stderr: "fatal: disk full".to_owned(),
        };
        let effects = qs.step(SaveDone(Err(SaveError::Git(failed))), &ask_me());
        let [ShowError(details)] = effects.as_slice() else {
            panic!("expected an error, got {effects:?}");
        };
        assert!(details.contains("disk full"), "{details}");
        assert!(!qs.is_busy());
    }

    #[test]
    fn a_failed_step_on_the_project_thread_ends_the_flow() {
        // The scan, applying the decisions and writing the identity report a failure as a
        // failed save.
        let ctx = ask_me();
        let mut qs = QuickSave::new();
        qs.step(Start { prefill: None }, &ctx);
        assert_eq!(
            qs.step(SaveDone(Err(SaveError::Git(GitError::Locked))), &ctx),
            [ShowLocked]
        );
        assert!(!qs.is_busy());

        let env = flag(".env", FlagReason::SecretName);
        qs.step(Start { prefill: None }, &ctx);
        qs.step(scanned(vec![env.clone()], false), &ctx);
        qs.step(WarningsAnswered(vec![(env, FlagAction::LeaveOut)]), &ctx);
        assert_eq!(
            qs.step(SaveDone(Err(SaveError::Git(GitError::Locked))), &ctx),
            [ShowLocked]
        );
        assert!(!qs.is_busy());

        // Nothing to save, found by the scan.
        qs.step(Start { prefill: None }, &ctx);
        assert_eq!(
            qs.step(SaveDone(Err(SaveError::NothingToSave)), &ctx),
            [Toast("save.nothing_new"), Refresh]
        );
        assert!(!qs.is_busy());
    }

    #[test]
    fn busy_ignores_start() {
        let ctx = ask_me();
        let mut qs = QuickSave::new();
        assert_eq!(qs.step(Start { prefill: None }, &ctx), [RunScan]);
        assert_eq!(qs.step(Start { prefill: None }, &ctx), []);
        assert!(qs.is_busy());
        // The flow under way goes on.
        assert_eq!(
            qs.step(scanned(vec![], false), &ctx),
            [name_dialog("", None)]
        );
        assert_eq!(
            qs.step(
                Start {
                    prefill: Some("other".to_owned())
                },
                &ctx
            ),
            []
        );
    }

    #[test]
    fn events_out_of_place_change_nothing() {
        let ctx = ask_me();
        let mut qs = QuickSave::new();
        assert_eq!(qs.step(NameConfirmed("x".to_owned()), &ctx), []);
        assert_eq!(qs.step(SaveDone(Ok(id())), &ctx), []);
        assert!(!qs.is_busy());
        qs.step(Start { prefill: None }, &ctx);
        assert_eq!(qs.step(IdentityGiven, &ctx), []);
        assert_eq!(qs.step(AiCancelRequested, &ctx), []);
        assert!(qs.is_busy());
    }
}
