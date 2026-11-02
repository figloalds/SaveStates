//! The name of a new save (spec §6.7): one field, prefilled (the AI's suggestion, or "Project
//! start") or empty. Enter or Save confirms, Escape or Cancel cancels, and Save stays off while
//! the name would be empty. A note line says why the AI gave no name.
//!
//! Like every dialog here, its content is built again each time it opens (Raven deletes it on
//! close) and its buttons fire actions bound once.

use std::cell::{Cell, RefCell};

use savestates_core::normalize_name;
use savestates_ui_raven::{Widget, Window, bind_action, dialog, tr};

use crate::dialogs::{after_close, build, part};
use crate::flows::quick_save::AiNote;
use crate::tr_key;

const FILE: &str = "dialog_name.xml";
/// The dialog's result when the user confirmed a name.
const CONFIRMED: i32 = 1;

/// The dialog while it is open.
struct Open {
    input: Widget,
    /// The name, once confirmed.
    name: Option<String>,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static BOUND: Cell<bool> = const { Cell::new(false) };
}

/// Asks for a name in `window`, starting with `prefill` (selected, so typing replaces it) and
/// showing the text of `note` when there is one. `on_answer` gets the name, cleaned up with
/// `normalize_name`, or `None` when the user cancelled.
pub fn show(
    window: Window,
    prefill: &str,
    note: Option<AiNote>,
    on_answer: impl FnOnce(Option<String>) + 'static,
) {
    bind_actions();
    let parts = build(FILE).and_then(|content| {
        let find = |name| part(content, FILE, name);
        Ok((
            content,
            find("name.input")?,
            find("name.save")?,
            find("name.note")?,
        ))
    });
    let (content, input, save, note_label) = match parts {
        Ok(parts) => parts,
        Err(error) => {
            log!("the name dialog could not be built: {error}");
            on_answer(None);
            return;
        }
    };
    input.text_input_set(prefill);
    save.set_enabled(normalize_name(prefill).is_some());
    input.on_text_changed(move |text| save.set_enabled(normalize_name(&text).is_some()));
    if let Some(note) = note {
        note_label.set_label_text(&tr(note_key(note)));
        note_label.set_visible(true);
    }
    OPEN.set(Some(Open { input, name: None }));
    dialog::open(window, content, move |result| {
        let name = OPEN.take().and_then(|open| open.name);
        after_close(move || on_answer(name.filter(|_| result == CONFIRMED)));
    });
    input.focus();
    input.text_input_select_all();
}

/// Binds the dialog's actions, once.
fn bind_actions() {
    if BOUND.replace(true) {
        return;
    }
    // Enter in the field, or Save: nothing happens while the name would be empty.
    bind_action("name.confirm", |sender| {
        let confirmed = OPEN.with_borrow_mut(|open| {
            let open = open.as_mut()?;
            open.name = Some(normalize_name(&open.input.text_input_get())?);
            Some(())
        });
        if confirmed.is_some() {
            dialog::close(sender, CONFIRMED);
        }
    });
    bind_action("name.cancel", |sender| dialog::close(sender, 0));
}

/// The text key of the note for an AI that gave no name.
fn note_key(note: AiNote) -> &'static str {
    match note {
        AiNote::NotInstalled => tr_key("ai.note.not_installed"),
        AiNote::NotLoggedIn => tr_key("ai.note.not_logged_in"),
        AiNote::Timeout => tr_key("ai.note.timeout"),
        AiNote::Failed => tr_key("ai.note.failed"),
        AiNote::Empty => tr_key("ai.note.empty"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_note_has_its_text() {
        let notes = [
            (AiNote::NotInstalled, "ai.note.not_installed"),
            (AiNote::NotLoggedIn, "ai.note.not_logged_in"),
            (AiNote::Timeout, "ai.note.timeout"),
            (AiNote::Failed, "ai.note.failed"),
            (AiNote::Empty, "ai.note.empty"),
        ];
        for (note, key) in notes {
            assert_eq!(note_key(note), key);
        }
    }
}
