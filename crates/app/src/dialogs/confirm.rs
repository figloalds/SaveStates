//! A question with translated buttons. Raven's own message box has fixed English buttons, so
//! every question the user sees goes through this dialog instead.
//!
//! Raven deletes a dialog's content when the dialog closes, so the content is built again from a
//! small XML each time. It holds no callback of its own: its buttons fire actions named after
//! their result (`confirm.answer.<n>`), each bound once, so a question leaks nothing but the
//! few bytes of the closed callback's slot (see `ui_raven::dialog::open`).

use std::cell::RefCell;
use std::collections::HashSet;

use savestates_ui_raven::{Window, bind_action, dialog, widget_from_xml};

thread_local! {
    /// The results whose action is bound already.
    static BOUND: RefCell<HashSet<i32>> = RefCell::new(HashSet::new());
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
    let content = match widget_from_xml(&dialog_xml(title_key, buttons)) {
        Ok(content) => content,
        Err(error) => {
            // Only a broken template gets here; answering "dismissed" keeps the caller going.
            eprintln!("the question dialog could not be built: {error}");
            on_answer(0);
            return;
        }
    };
    if let Some(text) = content.find("confirm.body") {
        text.set_label_text(body);
    }
    for &(_, result) in buttons {
        bind_answer(result);
    }
    dialog::open(window, content, on_answer);
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

/// The dialog's layout. The title and the labels are `@key` references that Raven translates.
fn dialog_xml(title_key: &str, buttons: &[(&str, i32)]) -> String {
    let mut row = String::new();
    for (i, &(label_key, result)) in buttons.iter().enumerate() {
        let style = if i == 0 { r#" Style="Primary""# } else { "" };
        row.push_str(&format!(
            r#"<Button Name="confirm.button.{i}" Text="@{label}" OnClicked="{action}"{style} MinWidth="96"/>"#,
            label = escape(label_key),
            action = answer_action(result),
        ));
    }
    format!(
        r##"<Border Background="#151d33" Padding="8,4" Width="460">
    <VBox Spacing="14">
        <Label Text="@{title}" TextSize="17"/>
        <Label Name="confirm.body" Wrap="true"/>
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
            &[("common.yes", 1), ("common.no", 0)],
        );
        assert!(xml.contains(r#"Text="@project.start_saving""#));
        assert!(xml.contains(
            r#"Name="confirm.button.0" Text="@common.yes" OnClicked="confirm.answer.1" Style="Primary""#
        ));
        assert!(xml.contains(
            r#"Name="confirm.button.1" Text="@common.no" OnClicked="confirm.answer.0" MinWidth"#
        ));
    }

    #[test]
    fn attribute_text_is_escaped() {
        assert_eq!(escape(r#"a"b<c>&'d"#), "a&quot;b&lt;c&gt;&amp;&apos;d");
    }
}
