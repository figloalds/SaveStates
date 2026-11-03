//! The name of a timeline (spec §5.3): one field, prefilled. Enter or Save confirms, Escape or
//! Cancel cancels. An empty name is allowed: it takes the timeline back to its original name.
//!
//! Like every dialog here, its content is built again each time it opens (Raven deletes it on
//! close) and its buttons fire actions bound once.

use std::cell::{Cell, RefCell};

use savestates_ui_raven::{Widget, Window, bind_action, dialog};

use crate::dialogs::{after_close, build, part};

const FILE: &str = "dialog_rename.xml";
/// The dialog's result when the user confirmed.
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

/// Asks for a name in `window`, starting with `prefill` (selected, so typing replaces it).
/// `on_answer` gets the name, trimmed (possibly empty), or `None` when the user cancelled.
pub fn show(window: Window, prefill: &str, on_answer: impl FnOnce(Option<String>) + 'static) {
    bind_actions();
    let parts = build(FILE).and_then(|content| Ok((content, part(content, FILE, "rename.input")?)));
    let (content, input) = match parts {
        Ok(parts) => parts,
        Err(error) => {
            log!("the rename dialog could not be built: {error}");
            on_answer(None);
            return;
        }
    };
    input.text_input_set(prefill);
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
    bind_action("rename.confirm", |sender| {
        OPEN.with_borrow_mut(|open| {
            if let Some(open) = open.as_mut() {
                open.name = Some(open.input.text_input_get().trim().to_owned());
            }
        });
        dialog::close(sender, CONFIRMED);
    });
    bind_action("rename.cancel", |sender| dialog::close(sender, 0));
}
