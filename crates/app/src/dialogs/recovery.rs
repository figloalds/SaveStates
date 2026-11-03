//! The Recovery list (spec §6.8.2): the work thrown away or set aside in the last 30 days,
//! newest first, each with when it was and its files, and a Restore button.
//!
//! Like every dialog here, its content is built again each time it opens (Raven deletes it on
//! close) and its buttons fire actions bound once.

use std::cell::{Cell, RefCell};

use chrono::{DateTime, FixedOffset, Local};
use savestates_core::{DeletedTimeline, RecoveryEntry};
use savestates_i18n::{Lang, relative_date};
use savestates_ui_raven::{
    Widget, Window, bind_action, dialog, tr_fmt, tr_plural, widget_from_xml,
};

use crate::dialogs::{after_close, build, part};
use crate::tr_key;

const FILE: &str = "dialog_recovery.xml";
/// The dialog's result when the user chose an entry to restore.
const RESTORE: i32 = 1;
/// How many file names an entry shows before "…".
const FILES_SHOWN: usize = 3;

/// One entry; its texts are set in code.
const ROW_XML: &str = r##"<Border Background="#0f1629" BorderColor="#2a3654" BorderWidth="1" CornerRadius="6" Padding="12,8">
    <HBox Spacing="12">
        <VBox Spacing="2" Slot.Fill="1">
            <Label Name="recovery.when"/>
            <Label Name="recovery.files" Class="muted" Wrap="true"/>
        </VBox>
        <Button Name="recovery.restore" OnClicked="recovery.restore" Slot.VAlign="Center"/>
    </HBox>
</Border>"##;

/// What the Recovery list holds: work set aside, or a timeline that was deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryItem {
    Work(RecoveryEntry),
    Timeline(DeletedTimeline),
}

impl RecoveryItem {
    /// When it was set aside or deleted, in Unix seconds.
    fn unix(&self) -> i64 {
        match self {
            Self::Work(entry) => entry.created_unix,
            Self::Timeline(deleted) => deleted.deleted_unix,
        }
    }
}

/// Both kinds together, newest first.
pub fn mix_items(work: Vec<RecoveryEntry>, deleted: Vec<DeletedTimeline>) -> Vec<RecoveryItem> {
    let mut items: Vec<RecoveryItem> = work
        .into_iter()
        .map(RecoveryItem::Work)
        .chain(deleted.into_iter().map(RecoveryItem::Timeline))
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.unix()));
    items
}

/// The dialog while it is open.
struct Open {
    items: Vec<RecoveryItem>,
    /// The Restore button of each item, in the same order.
    buttons: Vec<Widget>,
    /// The item to restore, once chosen.
    chosen: Option<RecoveryItem>,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static BOUND: Cell<bool> = const { Cell::new(false) };
}

/// Shows `items` (newest first) of the project named `project` in `window`, with dates in
/// `lang`. With `restore_off`, the Restore buttons are off and that text says why (a project
/// that can only be looked at). `on_restore` gets the item the user chose.
pub fn show(
    window: Window,
    project: &str,
    items: Vec<RecoveryItem>,
    lang: Lang,
    restore_off: Option<&str>,
    on_restore: impl FnOnce(RecoveryItem) + 'static,
) {
    bind_actions();
    let built = build(FILE).and_then(|content| {
        let find = |name| part(content, FILE, name);
        find("recovery.project")?.set_label_text(project);
        find("recovery.empty")?.set_visible(items.is_empty());
        find("recovery.scroll")?.set_visible(!items.is_empty());
        let rows = find("recovery.rows")?;
        let now = Local::now().fixed_offset();
        let buttons = items
            .iter()
            .map(|item| add_row(rows, item, lang, now, restore_off))
            .collect::<Result<Vec<_>, String>>()?;
        Ok((content, buttons))
    });
    let (content, buttons) = match built {
        Ok(built) => built,
        Err(error) => {
            log!("the recovery dialog could not be built: {error}");
            return;
        }
    };
    OPEN.set(Some(Open {
        items,
        buttons,
        chosen: None,
    }));
    dialog::open(window, content, move |result| {
        let chosen = OPEN.take().and_then(|open| open.chosen);
        if let Some(chosen) = chosen.filter(|_| result == RESTORE) {
            after_close(move || on_restore(chosen));
        }
    });
}

/// Adds the row of `item` to `rows`, and returns its Restore button.
fn add_row(
    rows: Widget,
    item: &RecoveryItem,
    lang: Lang,
    now: DateTime<FixedOffset>,
    restore_off: Option<&str>,
) -> Result<Widget, String> {
    let row = widget_from_xml(ROW_XML)?;
    let find = |name| part(row, "the recovery row", name);
    let (first, second) = item_lines(lang, item, now, &tr_plural, &tr_fmt);
    find("recovery.when")?.set_label_text(&first);
    find("recovery.files")?.set_label_text(&second);
    let restore = find("recovery.restore")?;
    restore.set_text_key("Text", tr_key("recovery.restore"));
    restore.set_enabled(restore_off.is_none());
    restore.set_tooltip(restore_off);
    rows.add_child(row);
    Ok(restore)
}

/// The two lines of a row. Work: when and how many files, then the first file names. A deleted
/// timeline: "Linha do tempo apagada: name", then when. `tr_plural` and `tr_fmt` give Raven's
/// texts in the app.
pub fn item_lines(
    lang: Lang,
    item: &RecoveryItem,
    now: DateTime<FixedOffset>,
    tr_plural: &dyn Fn(&str, i64) -> String,
    tr_fmt: &dyn Fn(&str, &[&str]) -> String,
) -> (String, String) {
    match item {
        RecoveryItem::Work(entry) => (
            entry_label(lang, entry, now, tr_plural),
            files_summary(&entry.files),
        ),
        RecoveryItem::Timeline(deleted) => {
            let name = deleted.display_name.as_deref().unwrap_or(&deleted.branch);
            let when = DateTime::from_timestamp(deleted.deleted_unix, 0)
                .map(|t| relative_date(lang, t.fixed_offset(), now))
                .unwrap_or_default();
            (tr_fmt("recovery.deleted_timeline", &[name]), when)
        }
    }
}

/// Binds the dialog's actions, once.
fn bind_actions() {
    if BOUND.replace(true) {
        return;
    }
    bind_action("recovery.restore", |sender| {
        let chosen = OPEN.with_borrow_mut(|open| {
            let open = open.as_mut()?;
            let i = open.buttons.iter().position(|&button| button == sender)?;
            open.chosen = Some(open.items.get(i)?.clone());
            Some(())
        });
        if chosen.is_some() {
            dialog::close(sender, RESTORE);
        }
    });
    bind_action("recovery.close", |sender| dialog::close(sender, 0));
}

/// An entry's line: when it was set aside ([`relative_date`]), then how many files it holds,
/// "hoje 10:42 · 3 arquivos". `tr_plural` gives the plural text of a key (Raven's, in the app).
pub fn entry_label(
    lang: Lang,
    e: &RecoveryEntry,
    now: DateTime<FixedOffset>,
    tr_plural: &dyn Fn(&str, i64) -> String,
) -> String {
    let count = i64::try_from(e.files.len()).unwrap_or(i64::MAX);
    let files = tr_plural("recovery.files", count);
    match DateTime::from_timestamp(e.created_unix, 0) {
        Some(t) => format!("{} · {files}", relative_date(lang, t.fixed_offset(), now)),
        None => files,
    }
}

/// The first few of `files`, separated by commas, with "…" when there are more.
fn files_summary(files: &[String]) -> String {
    let mut shown: Vec<&str> = files.iter().take(FILES_SHOWN).map(String::as_str).collect();
    if files.len() > FILES_SHOWN {
        shown.push("…");
    }
    shown.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(text: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(text).unwrap()
    }

    fn entry(created: DateTime<FixedOffset>, files: &[&str]) -> RecoveryEntry {
        RecoveryEntry {
            ref_name: format!("refs/savestates/recovery/{}-0", created.timestamp()),
            created_unix: created.timestamp(),
            files: files.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    fn fake_plural(key: &str, n: i64) -> String {
        format!("{key}:{n}")
    }

    fn deleted(unix: i64, branch: &str, name: Option<&str>) -> DeletedTimeline {
        DeletedTimeline {
            ref_name: format!("refs/savestates/deleted/{unix}-0"),
            branch: branch.to_owned(),
            display_name: name.map(str::to_owned),
            deleted_unix: unix,
        }
    }

    fn fake_fmt(key: &str, args: &[&str]) -> String {
        format!("{key}:{}", args.join("|"))
    }

    #[test]
    fn recovery_items_mix_both_kinds_newest_first() {
        let old = entry(at("2026-11-02T10:00:00-03:00"), &["a.txt"]);
        let new = entry(at("2026-11-02T12:00:00-03:00"), &["b.txt"]);
        let gone = deleted(
            at("2026-11-02T11:00:00-03:00").timestamp(),
            "savestates/x",
            None,
        );
        let items = mix_items(vec![old.clone(), new.clone()], vec![gone.clone()]);
        assert_eq!(
            items,
            vec![
                RecoveryItem::Work(new),
                RecoveryItem::Timeline(gone),
                RecoveryItem::Work(old),
            ]
        );
    }

    #[test]
    fn deleted_timeline_rows_name_the_timeline() {
        let now = at("2026-11-02T15:20:00-03:00");
        let at_ten = at("2026-11-02T10:42:00-03:00").timestamp();
        let named = RecoveryItem::Timeline(deleted(
            at_ten,
            "savestates/modo-escuro",
            Some("Modo escuro"),
        ));
        assert_eq!(
            item_lines(Lang::PtBr, &named, now, &fake_plural, &fake_fmt),
            (
                "recovery.deleted_timeline:Modo escuro".to_owned(),
                "hoje 10:42".to_owned()
            )
        );
        let plain = RecoveryItem::Timeline(deleted(at_ten, "savestates/tarde", None));
        assert_eq!(
            item_lines(Lang::PtBr, &plain, now, &fake_plural, &fake_fmt).0,
            "recovery.deleted_timeline:savestates/tarde"
        );
    }

    #[test]
    fn an_entry_says_when_and_how_many_files() {
        let now = at("2026-11-02T15:20:00-03:00");
        let e = entry(
            at("2026-11-02T10:42:00-03:00"),
            &["index.html", "css/estilo.css", "rascunho.txt"],
        );
        assert_eq!(
            entry_label(Lang::PtBr, &e, now, &fake_plural),
            "hoje 10:42 · recovery.files:3"
        );
        assert_eq!(
            entry_label(Lang::En, &e, now, &fake_plural),
            "today 10:42 · recovery.files:3"
        );
        // Seen from another offset, "today" is that offset's day.
        let utc = FixedOffset::east_opt(0).unwrap();
        let later = utc.with_ymd_and_hms(2026, 11, 3, 1, 0, 0).unwrap();
        assert_eq!(
            entry_label(Lang::PtBr, &e, later, &fake_plural),
            "ontem · recovery.files:3"
        );
    }

    #[test]
    fn an_entry_with_a_date_out_of_range_still_counts_its_files() {
        let now = at("2026-11-02T15:20:00-03:00");
        let e = RecoveryEntry {
            ref_name: "refs/savestates/recovery/x".to_owned(),
            created_unix: i64::MAX,
            files: vec!["a.txt".to_owned()],
        };
        assert_eq!(
            entry_label(Lang::PtBr, &e, now, &fake_plural),
            "recovery.files:1"
        );
    }

    #[test]
    fn the_files_of_an_entry_are_listed_briefly() {
        let now = at("2026-11-02T15:20:00-03:00");
        let few = entry(now, &["index.html", "css/estilo.css"]);
        assert_eq!(files_summary(&few.files), "index.html, css/estilo.css");
        let many = entry(now, &["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"]);
        assert_eq!(files_summary(&many.files), "a.txt, b.txt, c.txt, …");
        assert_eq!(files_summary(&[]), "");
    }
}
