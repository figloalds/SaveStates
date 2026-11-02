//! The warnings before a Quick Save (spec §6.8.1): one row per flagged file or folder, with its
//! path, why it was flagged, what to do about it (only the actions allowed for it, the
//! recommended one chosen), and a word when older saves already hold it.

use std::cell::{Cell, RefCell};

use savestates_core::{Flag, FlagAction, FlagReason, allowed_actions};
use savestates_ui_raven::{
    Widget, Window, bind_action, combo, dialog, tr, tr_fmt, widget_from_xml,
};

use crate::dialogs::{after_close, build, part};
use crate::tr_key;

const FILE: &str = "dialog_warnings.xml";
/// The dialog's result when the user chose Continue.
const CONTINUED: i32 = 1;
const BYTES_PER_MB: u64 = 1024 * 1024;

/// One row; its texts are set in code.
const ROW_XML: &str = r##"<Border Background="#0f1629" BorderColor="#2a3654" BorderWidth="1" CornerRadius="6" Padding="12,8">
    <VBox Spacing="6">
        <HBox Spacing="12">
            <VBox Spacing="2" Slot.Fill="1">
                <Label Name="warn.path" Wrap="true"/>
                <Label Name="warn.reason" Class="muted" Wrap="true"/>
            </VBox>
            <ComboBox Name="warn.action" MinWidth="270" Slot.VAlign="Center"/>
        </HBox>
        <Label Name="warn.older" Class="warning" Wrap="true" Visibility="Collapsed"/>
    </VBox>
</Border>"##;

/// The dialog while it is open.
struct Open {
    flags: Vec<Flag>,
    /// The action choice of each flag, in the same order.
    choices: Vec<Widget>,
    /// The decisions, once the user chose Continue.
    answer: Option<Vec<(Flag, FlagAction)>>,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static BOUND: Cell<bool> = const { Cell::new(false) };
}

/// Shows `flags` in `window`. `on_answer` gets each flag with the action chosen for it, or
/// `None` when the user cancelled.
pub fn show(
    window: Window,
    flags: Vec<Flag>,
    on_answer: impl FnOnce(Option<Vec<(Flag, FlagAction)>>) + 'static,
) {
    bind_actions();
    let built = build(FILE).and_then(|content| {
        let rows = part(content, FILE, "warn.rows")?;
        let choices = flags
            .iter()
            .map(|flag| add_row(rows, flag))
            .collect::<Result<Vec<_>, String>>()?;
        Ok((content, choices))
    });
    let (content, choices) = match built {
        Ok(built) => built,
        Err(error) => {
            eprintln!("the warnings dialog could not be built: {error}");
            on_answer(None);
            return;
        }
    };
    OPEN.set(Some(Open {
        flags,
        choices,
        answer: None,
    }));
    dialog::open(window, content, move |result| {
        let answer = OPEN.take().and_then(|open| open.answer);
        after_close(move || on_answer(answer.filter(|_| result == CONTINUED)));
    });
}

/// Adds the row of `flag` to `rows`, and returns its action choice.
fn add_row(rows: Widget, flag: &Flag) -> Result<Widget, String> {
    let row = widget_from_xml(ROW_XML)?;
    let find = |name| part(row, "the warning row", name);
    find("warn.path")?.set_label_text(&flag.path);
    find("warn.reason")?.set_label_text(&reason_text(&flag.reason));
    if flag.in_older_saves {
        let older = find("warn.older")?;
        older.set_label_text(&tr("warn.in_older_saves"));
        older.set_visible(true);
    }
    let choice = find("warn.action")?;
    let labels: Vec<String> = allowed_actions(flag)
        .into_iter()
        .map(|action| tr(action_key(action)))
        .collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    combo::set_items(choice, &labels);
    combo::set_selected(choice, 0);
    rows.add_child(row);
    Ok(choice)
}

/// Binds the dialog's actions, once.
fn bind_actions() {
    if BOUND.replace(true) {
        return;
    }
    bind_action("warn.continue", |sender| {
        OPEN.with_borrow_mut(|open| {
            if let Some(open) = open.as_mut() {
                let chosen: Vec<Option<usize>> = open
                    .choices
                    .iter()
                    .map(|&choice| combo::selected(choice))
                    .collect();
                open.answer = Some(decisions(&open.flags, &chosen));
            }
        });
        dialog::close(sender, CONTINUED);
    });
    bind_action("warn.cancel", |sender| dialog::close(sender, 0));
}

/// Each flag with the action at the index chosen for it among its allowed actions (the
/// recommended one when nothing is chosen).
fn decisions(flags: &[Flag], chosen: &[Option<usize>]) -> Vec<(Flag, FlagAction)> {
    flags
        .iter()
        .enumerate()
        .filter_map(|(i, flag)| {
            let allowed = allowed_actions(flag);
            let index = chosen.get(i).copied().flatten().unwrap_or(0);
            let action = allowed.get(index).or(allowed.first()).copied()?;
            Some((flag.clone(), action))
        })
        .collect()
}

/// Why a file was flagged, in plain words.
fn reason_text(reason: &FlagReason) -> String {
    match reason {
        FlagReason::SecretName => tr("warn.reason.secret_name"),
        FlagReason::SecretContent { line } => {
            tr_fmt("warn.reason.secret_content", &[&line.to_string()])
        }
        FlagReason::JunkFolder => tr("warn.reason.junk"),
        FlagReason::Large { bytes } => tr_fmt("warn.reason.large", &[&megabytes(*bytes)]),
    }
}

/// A size in whole megabytes (rounded), as the large-file limit counts them.
fn megabytes(bytes: u64) -> String {
    (bytes.saturating_add(BYTES_PER_MB / 2) / BYTES_PER_MB).to_string()
}

/// The text key of an action.
fn action_key(action: FlagAction) -> &'static str {
    match action {
        FlagAction::LeaveOut => tr_key("warn.action.leave_out"),
        FlagAction::StopSaving => tr_key("warn.action.stop_saving"),
        FlagAction::CancelAndFix => tr_key("warn.action.cancel_fix"),
        FlagAction::SaveAnyway => tr_key("warn.action.save_anyway"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(path: &str, reason: FlagReason, already_saved: bool) -> Flag {
        Flag {
            path: path.to_owned(),
            reason,
            already_saved,
            in_older_saves: false,
        }
    }

    #[test]
    fn the_choice_picks_among_the_allowed_actions() {
        let flags = [
            flag(".env", FlagReason::SecretName, false),
            flag("src/api.js", FlagReason::SecretContent { line: 12 }, true),
            flag("dist", FlagReason::JunkFolder, true),
            flag("video.mp4", FlagReason::Large { bytes: 40 << 20 }, false),
        ];
        let answered = decisions(&flags, &[Some(0), Some(1), Some(0), None]);
        let actions: Vec<FlagAction> = answered.iter().map(|(_, action)| *action).collect();
        assert_eq!(
            actions,
            [
                FlagAction::LeaveOut,
                FlagAction::SaveAnyway,
                FlagAction::StopSaving,
                // Nothing chosen: the recommended one.
                FlagAction::LeaveOut,
            ]
        );
        assert_eq!(answered[1].0, flags[1]);
        // A choice out of range, or a missing one, falls back to the recommended action.
        let answered = decisions(&flags[1..2], &[Some(7)]);
        assert_eq!(answered[0].1, FlagAction::CancelAndFix);
        assert_eq!(decisions(&flags[..1], &[])[0].1, FlagAction::LeaveOut);
    }

    #[test]
    fn sizes_are_in_whole_megabytes() {
        assert_eq!(megabytes(26 * BYTES_PER_MB), "26");
        assert_eq!(megabytes(25 * BYTES_PER_MB + BYTES_PER_MB / 2), "26");
        assert_eq!(megabytes(25 * BYTES_PER_MB + 1), "25");
        assert_eq!(megabytes(u64::MAX), (u64::MAX / BYTES_PER_MB).to_string());
    }

    #[test]
    fn every_action_has_its_label() {
        assert_eq!(action_key(FlagAction::LeaveOut), "warn.action.leave_out");
        assert_eq!(
            action_key(FlagAction::StopSaving),
            "warn.action.stop_saving"
        );
        assert_eq!(
            action_key(FlagAction::CancelAndFix),
            "warn.action.cancel_fix"
        );
        assert_eq!(
            action_key(FlagAction::SaveAnyway),
            "warn.action.save_anyway"
        );
    }
}
