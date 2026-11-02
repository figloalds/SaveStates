//! A question with translated buttons. Raven's own message box has fixed English buttons, so
//! every question the user sees goes through this dialog instead, and so does every message
//! ([`inform`]: one OK button).
//!
//! Raven deletes a dialog's content when the dialog closes, so the content is built again from a
//! small XML each time. It holds no callback of its own: its buttons fire actions named after
//! their result (`confirm.answer.<n>`), each bound once, so a question leaks nothing but the
//! few bytes of the closed callback's slot (see `ui_raven::dialog::open`).

use std::cell::RefCell;
use std::collections::HashSet;

use savestates_ui_raven::{Window, bind_action, dialog, widget_from_xml};

use crate::tr_key;

/// The most of a details text shown: a hook can print a whole lint report.
const MAX_DETAILS_BYTES: usize = 16 * 1024;

thread_local! {
    /// The results whose action is bound already.
    static BOUND: RefCell<HashSet<i32>> = RefCell::new(HashSet::new());
}

/// How a button looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Look {
    /// It stands out (filled with the accent color).
    Primary,
    Plain,
    /// Red text: it throws something away.
    Danger,
}

/// A button of the dialog: its label key, the result it answers, and how it looks.
#[derive(Clone, Copy, Debug)]
struct Button<'a> {
    label_key: &'a str,
    result: i32,
    look: Look,
}

/// Asks a question in `window`: a title (the translation of `title_key`), the text `body`
/// (already translated), and one button per `(label key, result)`, the first one standing out.
/// `on_answer` gets the result of the button clicked, or 0 when the user pressed Escape.
///
/// Mark the keys with `tr_key("…")` at the call site so the translation coverage check sees them.
pub fn confirm(
    window: Window,
    title_key: &str,
    body: &str,
    buttons: &[(&str, i32)],
    on_answer: impl FnOnce(i32) + 'static,
) {
    let buttons = plain_buttons(buttons);
    ask(window, title_key, body, None, &buttons, on_answer);
}

/// Asks before throwing something away: like [`confirm`], with the button that throws it away
/// (`discard`: its label key and result) in red, and Cancel as the one that stands out. Cancel
/// has the keyboard focus, so Enter cancels, as Escape does (both answer 0).
pub fn confirm_destructive(
    window: Window,
    title_key: &str,
    body: &str,
    discard: (&str, i32),
    on_answer: impl FnOnce(i32) + 'static,
) {
    let buttons = destructive_buttons(discard);
    ask(window, title_key, body, None, &buttons, on_answer);
}

/// Tells the user something in `window`, with an OK button: a title (the translation of
/// `title_key`), the text `body` (already translated) and, when given, technical `details`
/// (what a program printed) folded away under "Details".
pub fn inform(window: Window, title_key: &str, body: &str, details: Option<&str>) {
    let buttons = plain_buttons(&[(tr_key("common.ok"), 0)]);
    ask(window, title_key, body, details, &buttons, |_| {});
}

/// The buttons of a question: the first one stands out.
fn plain_buttons<'a>(buttons: &[(&'a str, i32)]) -> Vec<Button<'a>> {
    buttons
        .iter()
        .enumerate()
        .map(|(i, &(label_key, result))| Button {
            label_key,
            result,
            look: if i == 0 { Look::Primary } else { Look::Plain },
        })
        .collect()
}

/// The buttons of a question about throwing something away: `discard` in red, then Cancel,
/// which stands out.
fn destructive_buttons(discard: (&str, i32)) -> Vec<Button<'_>> {
    vec![
        Button {
            label_key: discard.0,
            result: discard.1,
            look: Look::Danger,
        },
        Button {
            label_key: tr_key("common.cancel"),
            result: 0,
            look: Look::Primary,
        },
    ]
}

/// Which button gets the keyboard focus, so that Enter presses it: Cancel, when a button throws
/// something away. Otherwise none (a dialog gives the focus to no widget by itself), and Enter
/// does nothing.
fn focused(buttons: &[Button]) -> Option<usize> {
    if !buttons.iter().any(|button| button.look == Look::Danger) {
        return None;
    }
    buttons
        .iter()
        .position(|button| button.look == Look::Primary)
}

fn ask(
    window: Window,
    title_key: &str,
    body: &str,
    details: Option<&str>,
    buttons: &[Button],
    on_answer: impl FnOnce(i32) + 'static,
) {
    let content = match widget_from_xml(&dialog_xml(title_key, buttons, details.is_some())) {
        Ok(content) => content,
        Err(error) => {
            // Only a broken template gets here; answering "dismissed" keeps the caller going.
            log!("the question dialog could not be built: {error}");
            on_answer(0);
            return;
        }
    };
    if let Some(text) = content.find("confirm.body") {
        text.set_label_text(body);
    }
    if let (Some(details), Some(text)) = (details, content.find("confirm.details_text")) {
        text.set_label_text(&shortened(details));
    }
    for button in buttons {
        bind_answer(button.result);
    }
    let focus = focused(buttons).and_then(|i| content.find(&format!("confirm.button.{i}")));
    dialog::open(window, content, on_answer);
    if let Some(button) = focus {
        button.focus();
    }
}

/// Binds the action of the buttons that answer `result`, once.
fn bind_answer(result: i32) {
    if BOUND.with_borrow_mut(|bound| bound.insert(result)) {
        bind_action(&answer_action(result), move |sender| {
            dialog::close(sender, result)
        });
    }
}

fn answer_action(result: i32) -> String {
    format!("confirm.answer.{result}")
}

/// `details` cut at [`MAX_DETAILS_BYTES`], with "…" where it was cut.
fn shortened(details: &str) -> String {
    let details = details.trim();
    if details.len() <= MAX_DETAILS_BYTES {
        return details.to_owned();
    }
    let kept = &details[..details.floor_char_boundary(MAX_DETAILS_BYTES)];
    format!("{kept}\n…")
}

/// The dialog's layout. The title and the labels are `@key` references that Raven translates.
/// With `details`, a folded "Details" section holds a text in a fixed-width font.
fn dialog_xml(title_key: &str, buttons: &[Button], details: bool) -> String {
    let mut row = String::new();
    for (i, button) in buttons.iter().enumerate() {
        let look = match button.look {
            Look::Primary => r#" Style="Primary""#,
            Look::Plain => "",
            Look::Danger => r#" Class="danger""#,
        };
        row.push_str(&format!(
            r#"<Button Name="confirm.button.{i}" Text="@{label}" OnClicked="{action}"{look} MinWidth="96"/>"#,
            label = escape(button.label_key),
            action = answer_action(button.result),
        ));
    }
    let details = if details {
        format!(
            r#"<Expander Name="confirm.details" Title="@{key}" Expanded="false"><ScrollBox MaxHeight="260"><Label Name="confirm.details_text" Font="monospace" Class="muted" Wrap="true"/></ScrollBox></Expander>"#,
            key = escape(tr_key("common.details")),
        )
    } else {
        String::new()
    };
    format!(
        r##"<Border Background="#151d33" Padding="8,4" Width="460">
    <VBox Spacing="14">
        <Label Text="@{title}" TextSize="17"/>
        <Label Name="confirm.body" Wrap="true"/>
        {details}
        <HBox Spacing="8"><Spacer Slot.Fill="1"/>{row}</HBox>
    </VBox>
</Border>"##,
        title = escape(title_key),
    )
}

/// `text` made safe inside an XML attribute value.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_refers_to_the_keys_and_the_answers() {
        let xml = dialog_xml(
            "project.start_saving",
            &plain_buttons(&[("common.yes", 1), ("common.no", 0)]),
            false,
        );
        assert!(xml.contains(r#"Text="@project.start_saving""#));
        assert!(xml.contains(
            r#"Name="confirm.button.0" Text="@common.yes" OnClicked="confirm.answer.1" Style="Primary""#
        ));
        assert!(xml.contains(
            r#"Name="confirm.button.1" Text="@common.no" OnClicked="confirm.answer.0" MinWidth"#
        ));
        assert!(!xml.contains("confirm.details_text"));
    }

    #[test]
    fn throwing_away_stands_out_as_danger_and_cancel_as_the_default() {
        let buttons = destructive_buttons(("throw_away.discard", 1));
        let xml = dialog_xml("throw_away.action", &buttons, false);
        assert!(xml.contains(
            r#"Name="confirm.button.0" Text="@throw_away.discard" OnClicked="confirm.answer.1" Class="danger" MinWidth"#
        ));
        assert!(xml.contains(
            r#"Name="confirm.button.1" Text="@common.cancel" OnClicked="confirm.answer.0" Style="Primary" MinWidth"#
        ));
        assert_eq!(xml.matches(r#"Style="Primary""#).count(), 1);
        // Enter presses the button with the keyboard focus: Cancel's.
        assert_eq!(focused(&buttons), Some(1));
        assert_eq!(
            focused(&plain_buttons(&[("common.yes", 1), ("common.no", 0)])),
            None
        );
    }

    #[test]
    fn details_are_folded_away_and_kept_short() {
        let xml = dialog_xml("save.quick", &plain_buttons(&[("common.ok", 0)]), true);
        assert!(xml.contains(
            r#"<Expander Name="confirm.details" Title="@common.details" Expanded="false">"#
        ));
        assert!(xml.contains(r#"Name="confirm.details_text""#));
        assert_eq!(shortened("  lint failed\n"), "lint failed");
        let long = "é".repeat(MAX_DETAILS_BYTES);
        let cut = shortened(&long);
        assert!(cut.len() <= MAX_DETAILS_BYTES + "\n…".len());
        assert!(cut.ends_with("é\n…"));
    }

    #[test]
    fn attribute_text_is_escaped() {
        assert_eq!(escape(r#"a"b<c>&'d"#), "a&quot;b&lt;c&gt;&amp;&apos;d");
    }
}
