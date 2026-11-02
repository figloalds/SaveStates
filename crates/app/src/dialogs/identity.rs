//! Who is saving (spec §6.8.3): asked once, before the first save on a computer where nobody
//! is set up. A name and an email, both needed (blank ones would not be written), and a note
//! that any email works.

use std::cell::{Cell, RefCell};

use savestates_ui_raven::{Widget, Window, bind_action, dialog};

use crate::dialogs::{after_close, build, part};

const FILE: &str = "dialog_identity.xml";
/// The dialog's result when the user confirmed.
const CONFIRMED: i32 = 1;

/// The dialog while it is open.
struct Open {
    name: Widget,
    email: Widget,
    /// The name and the email, once confirmed.
    answer: Option<(String, String)>,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static BOUND: Cell<bool> = const { Cell::new(false) };
}

/// Asks who is saving in `window`. `on_answer` gets the name and the email, trimmed and never
/// empty, or `None` when the user cancelled.
pub fn show(window: Window, on_answer: impl FnOnce(Option<(String, String)>) + 'static) {
    bind_actions();
    let parts = build(FILE).and_then(|content| {
        let find = |name| part(content, FILE, name);
        Ok((
            content,
            find("identity.name")?,
            find("identity.email")?,
            find("identity.continue")?,
        ))
    });
    let (content, name, email, button) = match parts {
        Ok(parts) => parts,
        Err(error) => {
            log!("the identity dialog could not be built: {error}");
            on_answer(None);
            return;
        }
    };
    for input in [name, email] {
        input.on_text_changed(move |_| {
            let filled = complete(&name.text_input_get(), &email.text_input_get()).is_some();
            button.set_enabled(filled);
        });
    }
    OPEN.set(Some(Open {
        name,
        email,
        answer: None,
    }));
    dialog::open(window, content, move |result| {
        let answer = OPEN.take().and_then(|open| open.answer);
        after_close(move || on_answer(answer.filter(|_| result == CONFIRMED)));
    });
    name.focus();
}

/// Binds the dialog's actions, once.
fn bind_actions() {
    if BOUND.replace(true) {
        return;
    }
    // Continue, or Enter in either field: with a field still empty, the keyboard goes there.
    bind_action("identity.confirm", |sender| {
        let confirmed = OPEN.with_borrow_mut(|open| {
            let open = open.as_mut()?;
            let (name, email) = (open.name.text_input_get(), open.email.text_input_get());
            match complete(&name, &email) {
                Some(answer) => {
                    open.answer = Some(answer);
                    Some(true)
                }
                None if name.trim().is_empty() => {
                    open.name.focus();
                    Some(false)
                }
                None => {
                    open.email.focus();
                    Some(false)
                }
            }
        });
        if confirmed == Some(true) {
            dialog::close(sender, CONFIRMED);
        }
    });
    bind_action("identity.cancel", |sender| dialog::close(sender, 0));
}

/// The name and the email, trimmed, when neither is empty.
fn complete(name: &str, email: &str) -> Option<(String, String)> {
    let (name, email) = (name.trim(), email.trim());
    (!name.is_empty() && !email.is_empty()).then(|| (name.to_owned(), email.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_are_needed_and_trimmed() {
        assert_eq!(
            complete("  Ana Souza ", " ana@example.com\t"),
            Some(("Ana Souza".to_owned(), "ana@example.com".to_owned()))
        );
        assert_eq!(complete("Ana", "   "), None);
        assert_eq!(complete(" ", "ana@example.com"), None);
        assert_eq!(complete("", ""), None);
    }
}
