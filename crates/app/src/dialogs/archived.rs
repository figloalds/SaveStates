//! The archived timelines (spec §7.2): each with its color, name, how many saves only it has and
//! its last save, and the buttons Restaurar and Apagar….
//!
//! Like every dialog here, its content is built again each time it opens (Raven deletes it on
//! close) and its buttons fire actions bound once.

use std::cell::{Cell, RefCell};

use chrono::{DateTime, FixedOffset, Local};
use savestates_core::ArchivedTimeline;
use savestates_i18n::{Lang, relative_date};
use savestates_ui_raven::{Rgba, Widget, Window, bind_action, dialog, tr_plural, widget_from_xml};

use crate::dialogs::{after_close, build, part};
use crate::{theme, tr_key};

const FILE: &str = "dialog_archived.xml";
/// The dialog's results when the user chose a timeline to restore, or to delete.
const RESTORE: i32 = 1;
const DELETE: i32 = 2;

/// What the user chose, with the git name of the timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchivedChoice {
    Restore(String),
    Delete(String),
}

/// One row; its texts are set in code, and the dot has the timeline's color.
fn row_xml(color: Rgba) -> String {
    format!(
        r##"<Border Background="#0f1629" BorderColor="#2a3654" BorderWidth="1" CornerRadius="6" Padding="12,8">
    <HBox Spacing="12">
        <Border Background="{}" CornerRadius="6" Width="12" Height="12" Slot.VAlign="Center"/>
        <VBox Spacing="2" Slot.Fill="1">
            <Label Name="archived.name"/>
            <Label Name="archived.detail" Class="muted" Wrap="true"/>
        </VBox>
        <Button Name="archived.restore" OnClicked="archived.restore" Slot.VAlign="Center"/>
        <Button Name="archived.delete" OnClicked="archived.delete" Slot.VAlign="Center"/>
    </HBox>
</Border>"##,
        hex_code(color)
    )
}

/// `#rrggbb` of an opaque color.
fn hex_code(c: Rgba) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(c.0), byte(c.1), byte(c.2))
}

/// A row's buttons and what they act on.
struct RowButtons {
    branch: String,
    restore: Widget,
    delete: Widget,
}

/// The dialog while it is open.
struct Open {
    rows: Vec<RowButtons>,
    chosen: Option<ArchivedChoice>,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static BOUND: Cell<bool> = const { Cell::new(false) };
}

/// Shows the archived `rows` of the project named `project` in `window`, with dates in `lang`.
/// With `read_only`, the buttons are off and that text says why. `on_choice` gets what the user
/// chose.
pub fn show(
    window: Window,
    project: &str,
    rows: Vec<ArchivedTimeline>,
    lang: Lang,
    read_only: Option<&str>,
    on_choice: impl FnOnce(ArchivedChoice) + 'static,
) {
    bind_actions();
    let built = build(FILE).and_then(|content| {
        let find = |name| part(content, FILE, name);
        find("archived.project")?.set_label_text(project);
        find("archived.empty")?.set_visible(rows.is_empty());
        find("archived.scroll")?.set_visible(!rows.is_empty());
        let list = find("archived.rows")?;
        let now = Local::now().fixed_offset();
        let buttons = rows
            .iter()
            .map(|row| add_row(list, row, lang, now, read_only))
            .collect::<Result<Vec<_>, String>>()?;
        Ok((content, buttons))
    });
    let (content, buttons) = match built {
        Ok(built) => built,
        Err(error) => {
            log!("the archived timelines dialog could not be built: {error}");
            return;
        }
    };
    OPEN.set(Some(Open {
        rows: buttons,
        chosen: None,
    }));
    dialog::open(window, content, move |result| {
        let chosen = OPEN.take().and_then(|open| open.chosen);
        if let Some(chosen) = chosen.filter(|_| result == RESTORE || result == DELETE) {
            after_close(move || on_choice(chosen));
        }
    });
}

/// Adds the row of `archived` to `list`, and returns its buttons.
fn add_row(
    list: Widget,
    archived: &ArchivedTimeline,
    lang: Lang,
    now: DateTime<FixedOffset>,
    read_only: Option<&str>,
) -> Result<RowButtons, String> {
    let color = theme::lane_color(&archived.branch, false);
    let row = widget_from_xml(&row_xml(color))?;
    let find = |name| part(row, "the archived row", name);
    let (name, detail) = row_lines(lang, archived, now, &tr_plural);
    find("archived.name")?.set_label_text(&name);
    find("archived.detail")?.set_label_text(&detail);
    let restore = find("archived.restore")?;
    let delete = find("archived.delete")?;
    restore.set_text_key("Text", tr_key("archive.restore"));
    delete.set_text_key("Text", tr_key("archive.delete"));
    for button in [restore, delete] {
        button.set_enabled(read_only.is_none());
        button.set_tooltip(read_only);
    }
    list.add_child(row);
    Ok(RowButtons {
        branch: archived.branch.clone(),
        restore,
        delete,
    })
}

/// The two lines of a row: the name (the git name when it has none of its own), then how many
/// saves it has and when the last was made, "3 saves · hoje 10:42".
pub fn row_lines(
    lang: Lang,
    archived: &ArchivedTimeline,
    now: DateTime<FixedOffset>,
    tr_plural: &dyn Fn(&str, i64) -> String,
) -> (String, String) {
    let name = archived
        .display_name
        .clone()
        .unwrap_or_else(|| archived.branch.clone());
    let count = i64::try_from(archived.own_saves).unwrap_or(i64::MAX);
    let when = relative_date(lang, archived.last_save.committed, now);
    (
        name,
        format!("{} · {when}", tr_plural("timeline.count", count)),
    )
}

/// Binds the dialog's actions, once.
fn bind_actions() {
    if BOUND.replace(true) {
        return;
    }
    bind_action("archived.restore", |sender| choose(sender, true));
    bind_action("archived.delete", |sender| choose(sender, false));
    bind_action("archived.close", |sender| dialog::close(sender, 0));
}

/// A button of a row was clicked.
fn choose(sender: Widget, restore: bool) {
    let chosen = OPEN.with_borrow_mut(|open| {
        let open = open.as_mut()?;
        let row = open.rows.iter().find(|row| {
            if restore {
                row.restore == sender
            } else {
                row.delete == sender
            }
        })?;
        let branch = row.branch.clone();
        open.chosen = Some(if restore {
            ArchivedChoice::Restore(branch)
        } else {
            ArchivedChoice::Delete(branch)
        });
        Some(())
    });
    if chosen.is_some() {
        dialog::close(sender, if restore { RESTORE } else { DELETE });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use savestates_core::{SaveId, SaveState};

    fn at(text: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(text).unwrap()
    }

    fn fake_plural(key: &str, n: i64) -> String {
        format!("{key}:{n}")
    }

    fn archived(name: Option<&str>) -> ArchivedTimeline {
        let time = at("2026-11-02T10:42:00-03:00");
        ArchivedTimeline {
            branch: "savestates/modo-escuro".to_owned(),
            display_name: name.map(str::to_owned),
            own_saves: 3,
            last_save: SaveState {
                id: SaveId("a".repeat(40)),
                name: "x".to_owned(),
                time,
                committed: time,
                files_changed: 1,
            },
        }
    }

    #[test]
    fn archived_rows_show_count_and_last_save() {
        let now = at("2026-11-02T15:20:00-03:00");
        assert_eq!(
            row_lines(
                Lang::PtBr,
                &archived(Some("Modo escuro")),
                now,
                &fake_plural
            ),
            (
                "Modo escuro".to_owned(),
                "timeline.count:3 · hoje 10:42".to_owned()
            )
        );
        // Without a name of its own, the git name.
        assert_eq!(
            row_lines(Lang::PtBr, &archived(None), now, &fake_plural).0,
            "savestates/modo-escuro"
        );
    }

    #[test]
    fn the_dot_has_the_color_of_the_timeline() {
        assert_eq!(hex_code(theme::LINE), "#38bdf8");
    }
}
