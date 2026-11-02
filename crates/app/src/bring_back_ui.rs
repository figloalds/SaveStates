//! Bring back (spec §6.6): the buttons under a peeked file, and what they and the banner's
//! "everything" button do.
//!
//! Every operation runs on the tab's worker and ends with a refresh, so that "Now" shows what
//! came back. Work that had to be set aside for it goes to the Recovery list, and a toast offers
//! to undo.

use savestates_core::{
    BringBackError, Change, ChangeKind, CoreError, Project, RecoveryEntry, SaveId, Side,
    UnsavedPolicy, bring_back_everything as bring_back_all, bring_back_file as bring_back_one,
    restore_recovery,
};
use savestates_ui_raven::{Widget, Window, tr};

use crate::app::{with_app, with_tab};
use crate::dialogs::confirm;
use crate::tabs::TabId;
use crate::worker::when_normal;
use crate::{toast, tr_key};

/// How long the toast that offers to undo stays up.
const UNDO_SECONDS: f64 = 10.0;
/// How long a toast that only informs stays up.
const NOTICE_SECONDS: f64 = 8.0;

/// The answers of the question asked before bringing everything back over unsaved changes.
const QUICK_SAVE_FIRST: i32 = 2;
const ANYWAY: i32 = 1;
const YES: i32 = 1;

/// A button under a file in peek's "What changed" tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BringBackButton {
    /// The file as it was in the peeked save.
    After,
    /// The file as it was just before the peeked save (what undoes a save that broke it).
    Before,
    /// "Before", for a file the save created: bringing that back deletes the file.
    BeforeDeletes,
}

/// The buttons for a file that the peeked save changed in this way.
pub fn bring_back_buttons(kind: &ChangeKind) -> Vec<BringBackButton> {
    match kind {
        ChangeKind::Changed | ChangeKind::Renamed { .. } => {
            vec![BringBackButton::After, BringBackButton::Before]
        }
        ChangeKind::New => vec![BringBackButton::After, BringBackButton::BeforeDeletes],
        ChangeKind::Deleted => vec![BringBackButton::Before],
    }
}

/// Where `button` brings `change` (a change the save `save` made) back from: the side, and the
/// path the file has there. Before a rename, the file had its old name.
pub fn file_source(button: BringBackButton, save: &SaveId, change: &Change) -> (Side, String) {
    match (button, &change.kind) {
        (BringBackButton::After, _) => (Side::Save(save.clone()), change.path.clone()),
        (
            BringBackButton::Before | BringBackButton::BeforeDeletes,
            ChangeKind::Renamed { from },
        ) => (Side::Before(save.clone()), from.clone()),
        (BringBackButton::Before | BringBackButton::BeforeDeletes, _) => {
            (Side::Before(save.clone()), change.path.clone())
        }
    }
}

/// Whether the file at `path` has unsaved changes among `unsaved` (under either name, for a
/// rename), so that bringing it back asks first. Letter case is ignored, as Windows does.
pub fn has_unsaved_changes(path: &str, unsaved: &[Change]) -> bool {
    let wanted = path.to_lowercase();
    unsaved.iter().any(|change| {
        let from = match &change.kind {
            ChangeKind::Renamed { from } => Some(from.as_str()),
            _ => None,
        };
        std::iter::once(change.path.as_str())
            .chain(from)
            .any(|name| name.to_lowercase() == wanted)
    })
}

/// The bring-back buttons under the comparison view (`project_tab.xml`): the ones of "What
/// changed", and "Bring back this file" of "All files".
pub struct BringBackRow {
    row: Widget,
    after: Widget,
    before: Widget,
    before_deletes: Widget,
    this_file: Widget,
}

impl BringBackRow {
    pub fn new(page: Widget) -> Result<BringBackRow, String> {
        let find = |name: &str| {
            page.find(name)
                .ok_or_else(|| format!("project_tab.xml has no widget named {name}"))
        };
        Ok(BringBackRow {
            row: find("bring_back.row")?,
            after: find("bring_back.button.after")?,
            before: find("bring_back.button.before")?,
            before_deletes: find("bring_back.button.before_deletes")?,
            this_file: find("bring_back.button.this_file")?,
        })
    }

    pub fn hide(&self) {
        self.row.set_visible(false);
    }

    /// Shows the buttons for a file the peeked save changed in this way.
    pub fn show_for(&self, kind: &ChangeKind) {
        let shown = bring_back_buttons(kind);
        for (widget, button) in self.buttons() {
            widget.set_visible(shown.contains(&button));
        }
        self.this_file.set_visible(false);
        self.row.set_visible(true);
    }

    /// Shows "Bring back this file" alone.
    pub fn show_this_file(&self) {
        for (widget, _) in self.buttons() {
            widget.set_visible(false);
        }
        self.this_file.set_visible(true);
        self.row.set_visible(true);
    }

    /// Which button `sender` is: one of "What changed" (`Some`), or "Bring back this file"
    /// (`None` inside).
    pub fn which(&self, sender: Widget) -> Option<Option<BringBackButton>> {
        if sender == self.this_file {
            return Some(None);
        }
        self.buttons()
            .into_iter()
            .find_map(|(widget, button)| (widget == sender).then_some(Some(button)))
    }

    /// Every button, to turn on or off together.
    pub fn widgets(&self) -> [Widget; 4] {
        [self.after, self.before, self.before_deletes, self.this_file]
    }

    fn buttons(&self) -> [(Widget, BringBackButton); 3] {
        [
            (self.after, BringBackButton::After),
            (self.before, BringBackButton::Before),
            (self.before_deletes, BringBackButton::BeforeDeletes),
        ]
    }
}

/// Brings one file back into the project folder of the tab `tab`: `path` as it was on `side`.
/// When the file has unsaved changes (`ask_first`), asks first; they go to the Recovery list.
pub fn bring_back_file(window: Window, tab: TabId, side: Side, path: String, ask_first: bool) {
    if !ask_first {
        run_file(tab, side, path);
        return;
    }
    confirm(
        window,
        tr_key("bring_back.action"),
        &tr("bring_back.confirm_overwrite"),
        &[(tr_key("common.yes"), YES), (tr_key("common.no"), 0)],
        move |answer| {
            if answer == YES {
                run_file(tab, side, path);
            }
        },
    );
}

fn run_file(tab: TabId, side: Side, path: String) {
    with_tab(tab, |t| {
        t.submit(move |p| {
            let result = file_job(p, &side, &path);
            Box::new(move || match result {
                Some(result) => finished(tab, result),
                None => with_tab(tab, |t| t.refresh()),
            })
        });
    });
}

/// Brings `path` back as it was on `side`, on the project's thread. `None`: the project could
/// only be looked at by then, and nothing was touched.
fn file_job(
    p: &Project,
    side: &Side,
    path: &str,
) -> Option<Result<Option<RecoveryEntry>, CoreError>> {
    when_normal(p, |p| bring_back_one(p, side, path))
}

/// Makes the project folder of the tab `tab` match the save `save`. Over unsaved changes
/// (`has_unsaved`), asks first whether to Quick Save them, set them aside, or cancel.
pub fn bring_back_everything(window: Window, tab: TabId, save: SaveId, has_unsaved: bool) {
    if has_unsaved {
        ask_about_unsaved(window, tab, save);
    } else {
        run_everything(tab, save, UnsavedPolicy::RequireClean);
    }
}

fn ask_about_unsaved(window: Window, tab: TabId, save: SaveId) {
    confirm(
        window,
        tr_key("bring_back.everything"),
        &tr("bring_back.unsaved_question"),
        &[
            (tr_key("bring_back.quick_save_first"), QUICK_SAVE_FIRST),
            (tr_key("bring_back.anyway"), ANYWAY),
            (tr_key("common.cancel"), 0),
        ],
        move |answer| match answer {
            QUICK_SAVE_FIRST => with_app(|app| {
                let then = Box::new(move || run_everything(tab, save, UnsavedPolicy::RequireClean));
                app.quick_save_then(tab, then);
            }),
            ANYWAY => run_everything(tab, save, UnsavedPolicy::MoveToRecovery),
            _ => {}
        },
    );
}

fn run_everything(tab: TabId, save: SaveId, policy: UnsavedPolicy) {
    with_tab(tab, |t| {
        t.submit(move |p| {
            let result = everything_job(p, &save, policy);
            Box::new(move || match result {
                // Changes appeared since the question was (not) asked: ask now.
                Some(Err(BringBackError::HasUnsavedChanges)) => {
                    with_app(|app| ask_about_unsaved(app.window(), tab, save));
                    with_tab(tab, |t| t.refresh());
                }
                Some(Err(BringBackError::Core(error))) => finished(tab, Err(error)),
                Some(Ok(entry)) => finished(tab, Ok(entry)),
                None => with_tab(tab, |t| t.refresh()),
            })
        });
    });
}

/// Makes the project folder match the save `save`, on the project's thread. `None`: the
/// project could only be looked at by then, and nothing was touched.
fn everything_job(
    p: &Project,
    save: &SaveId,
    policy: UnsavedPolicy,
) -> Option<Result<Option<RecoveryEntry>, BringBackError>> {
    when_normal(p, |p| bring_back_all(p, save, policy))
}

/// A bring back is done: says so (offering to undo what was set aside), and refreshes the tab.
fn finished(tab: TabId, result: Result<Option<RecoveryEntry>, CoreError>) {
    match result {
        Ok(Some(entry)) => offer_undo(tab, tr_key("recovery.moved"), entry),
        Ok(None) => toast::show(tr_key("bring_back.done"), None, NOTICE_SECONDS),
        Err(error) => {
            log!("bringing back failed: {error}");
            toast::show(tr_key("bring_back.failed"), None, NOTICE_SECONDS);
        }
    }
    with_tab(tab, |t| t.refresh());
}

/// Says the text of `text_key` (that work went to the Recovery list as `entry`) for 10 seconds,
/// with an Undo that puts `entry` back in the project folder of the tab `tab`.
pub fn offer_undo(tab: TabId, text_key: &'static str, entry: RecoveryEntry) {
    let ref_name = entry.ref_name;
    let undo: Box<dyn FnOnce()> = Box::new(move || {
        with_tab(tab, |t| {
            t.submit(move |p| {
                let result = undo_job(p, &ref_name);
                Box::new(move || match result {
                    Some(result) => undone(tab, result),
                    None => with_tab(tab, |t| t.refresh()),
                })
            });
        });
    });
    toast::show(text_key, Some((tr_key("common.undo"), undo)), UNDO_SECONDS);
}

/// Puts the Recovery entry `ref_name` back in the project folder, on the project's thread.
/// `None`: the project could only be looked at by then, and nothing was touched.
fn undo_job(p: &Project, ref_name: &str) -> Option<Result<Option<RecoveryEntry>, CoreError>> {
    when_normal(p, |p| restore_recovery(p, ref_name))
}

/// Undo is done. Putting the changes back may itself set aside what was there meanwhile: that
/// can be undone too.
fn undone(tab: TabId, result: Result<Option<RecoveryEntry>, CoreError>) {
    match result {
        Ok(Some(entry)) => offer_undo(tab, tr_key("recovery.moved"), entry),
        Ok(None) => {}
        Err(error) => {
            log!("undoing failed: {error}");
            toast::show(tr_key("recovery.undo_failed"), None, NOTICE_SECONDS);
        }
    }
    with_tab(tab, |t| t.refresh());
}

#[cfg(test)]
mod tests {
    use super::*;
    use BringBackButton::{After, Before, BeforeDeletes};
    use savestates_core::{Change, ChangeKind, SaveId, Side};

    #[test]
    fn the_buttons_follow_what_the_save_did_to_the_file() {
        assert_eq!(bring_back_buttons(&ChangeKind::Changed), [After, Before]);
        let renamed = ChangeKind::Renamed {
            from: "css/antigo.css".to_owned(),
        };
        assert_eq!(bring_back_buttons(&renamed), [After, Before]);
        // A file this save created: "Before" deletes it.
        assert_eq!(bring_back_buttons(&ChangeKind::New), [After, BeforeDeletes]);
        // A file this save deleted has no "After" to bring back.
        assert_eq!(bring_back_buttons(&ChangeKind::Deleted), [Before]);
    }

    fn change(path: &str, kind: ChangeKind) -> Change {
        Change {
            path: path.to_owned(),
            kind,
        }
    }

    #[test]
    fn each_button_reads_the_file_where_it_was() {
        let id = SaveId("a".repeat(40));
        let changed = change("css/estilo.css", ChangeKind::Changed);
        assert_eq!(
            file_source(After, &id, &changed),
            (Side::Save(id.clone()), "css/estilo.css".to_owned())
        );
        assert_eq!(
            file_source(Before, &id, &changed),
            (Side::Before(id.clone()), "css/estilo.css".to_owned())
        );
        let created = change("rascunho.txt", ChangeKind::New);
        assert_eq!(
            file_source(BeforeDeletes, &id, &created),
            (Side::Before(id.clone()), "rascunho.txt".to_owned())
        );
        // Before the save, a renamed file still had its old name.
        let renamed = change(
            "css/novo.css",
            ChangeKind::Renamed {
                from: "css/antigo.css".to_owned(),
            },
        );
        assert_eq!(
            file_source(Before, &id, &renamed),
            (Side::Before(id.clone()), "css/antigo.css".to_owned())
        );
        assert_eq!(
            file_source(After, &id, &renamed),
            (Side::Save(id), "css/novo.css".to_owned())
        );
    }

    #[test]
    fn only_a_file_with_unsaved_changes_asks_first() {
        let unsaved = [
            change("index.html", ChangeKind::Changed),
            change("rascunho.txt", ChangeKind::New),
            change(
                "css/novo.css",
                ChangeKind::Renamed {
                    from: "css/velho.css".to_owned(),
                },
            ),
        ];
        assert!(has_unsaved_changes("index.html", &unsaved));
        assert!(has_unsaved_changes("rascunho.txt", &unsaved));
        // A rename counts under both of its names.
        assert!(has_unsaved_changes("css/novo.css", &unsaved));
        assert!(has_unsaved_changes("css/velho.css", &unsaved));
        // Windows does not tell letter case apart in file names.
        assert!(has_unsaved_changes("INDEX.html", &unsaved));
        assert!(!has_unsaved_changes("sobre.html", &unsaved));
        assert!(!has_unsaved_changes("index.htm", &unsaved));
        assert!(!has_unsaved_changes("index.html", &[]));
    }

    // testkit is safe in this crate's unit tests: the app is a binary that nothing depends on,
    // so its tests and testkit share the one build of savestates-git.
    mod when_another_program_starts_a_merge {
        use super::super::*;
        use savestates_core::{OpenOutcome, Scope, list_recovery, open_project, throw_away};
        use testkit::TestRepo;

        /// A project with two saves of `a.txt` ("one", then "two"), the first one's id, and the
        /// project opened.
        fn two_saves() -> (TestRepo, SaveId, Project) {
            let repo = TestRepo::new();
            repo.write("a.txt", "one\n");
            let first = SaveId(repo.commit_all("first"));
            repo.write("a.txt", "two\n");
            repo.commit_all("second");
            let p = match open_project(&repo.exe(), repo.path()).unwrap() {
                OpenOutcome::Opened(p) => p,
                other => panic!("expected a project, got {other:?}"),
            };
            (repo, first, p)
        }

        /// What another program does after the last refresh, while the user clicks.
        fn start_merge(repo: &TestRepo) {
            let head = repo.git(&["rev-parse", "HEAD"]);
            repo.write(".git/MERGE_HEAD", head);
        }

        fn a_txt(repo: &TestRepo) -> String {
            std::fs::read_to_string(repo.path().join("a.txt")).unwrap()
        }

        #[test]
        fn bringing_back_a_file_touches_nothing() {
            let (repo, first, p) = two_saves();
            start_merge(&repo);
            assert!(file_job(&p, &Side::Save(first), "a.txt").is_none());
            assert_eq!(a_txt(&repo), "two\n");
        }

        #[test]
        fn bringing_back_everything_touches_nothing() {
            let (repo, first, p) = two_saves();
            start_merge(&repo);
            assert!(everything_job(&p, &first, UnsavedPolicy::MoveToRecovery).is_none());
            assert_eq!(a_txt(&repo), "two\n");
        }

        #[test]
        fn undo_touches_nothing() {
            let (repo, _, p) = two_saves();
            repo.write("a.txt", "unsaved\n");
            let entry = throw_away(&p, &Scope::All).unwrap().expect("an entry");
            assert_eq!(a_txt(&repo), "two\n");
            start_merge(&repo);

            assert!(undo_job(&p, &entry.ref_name).is_none());
            assert_eq!(a_txt(&repo), "two\n");
            assert_eq!(list_recovery(&p).unwrap(), [entry], "still in Recovery");
        }

        #[test]
        fn a_project_saved_to_as_usual_is_changed() {
            let (repo, first, p) = two_saves();
            let result = file_job(&p, &Side::Save(first), "a.txt");
            assert!(matches!(result, Some(Ok(None))), "{result:?}");
            assert_eq!(a_txt(&repo), "one\n");
        }
    }
}
