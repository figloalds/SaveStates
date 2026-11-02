//! A short message at the bottom right of the main window, with an optional action ("Undo").
//! One at a time: a new one replaces the one showing.

use std::cell::RefCell;

use savestates_ui_raven::{Timer, Widget, Window, bind_action, task};

struct Toast {
    root: Widget,
    text: Widget,
    action: Widget,
    /// What the action button does, until it is clicked or the toast goes away.
    on_action: Option<Box<dyn FnOnce()>>,
    timer: Option<Timer>,
    /// Counts the toasts shown, so that the timer of a replaced one hides nothing.
    shown: u64,
}

thread_local! {
    /// The main window's toast; only the UI thread's is ever set.
    static TOAST: RefCell<Option<Toast>> = const { RefCell::new(None) };
}

/// Finds the toast in `window` (`main.xml`) and wires its action button. Call once.
pub fn install(window: Window) {
    let (Some(root), Some(text), Some(action)) = (
        window.find("toast"),
        window.find("toast.text"),
        window.find("toast.action"),
    ) else {
        return;
    };
    TOAST.set(Some(Toast {
        root,
        text,
        action,
        on_action: None,
        timer: None,
        shown: 0,
    }));
    bind_action("toast.action", |_| {
        // Take the action out first: it may show another toast.
        let on_action = TOAST.with_borrow_mut(|toast| toast.as_mut().and_then(hide));
        if let Some(on_action) = on_action {
            on_action();
        }
    });
}

/// Shows `text` for `seconds`, with a button labelled with the action's text that runs it once
/// (the toast goes away when it is clicked).
pub fn show(text: &str, action: Option<(String, Box<dyn FnOnce()>)>, seconds: f64) {
    let shown = TOAST.with_borrow_mut(|toast| {
        let toast = toast.as_mut()?;
        hide(toast);
        toast.shown += 1;
        toast.text.set_label_text(text);
        toast.action.set_visible(action.is_some());
        toast.on_action = action.map(|(label, on_action)| {
            toast.action.set_button_text(&label);
            on_action
        });
        toast.root.set_visible(true);
        Some(toast.shown)
    });
    let Some(shown) = shown else { return };
    let timer = task::timer(seconds.max(0.0), false, move || {
        TOAST.with_borrow_mut(|toast| {
            if let Some(toast) = toast.as_mut().filter(|t| t.shown == shown) {
                toast.timer = None;
                hide(toast);
            }
        });
    });
    TOAST.with_borrow_mut(|toast| {
        if let Some(toast) = toast.as_mut().filter(|t| t.shown == shown) {
            toast.timer = Some(timer);
        }
    });
}

/// Hides the toast and gives back its action, which will not run otherwise.
fn hide(toast: &mut Toast) -> Option<Box<dyn FnOnce()>> {
    if let Some(timer) = toast.timer.take() {
        timer.cancel();
    }
    toast.root.set_visible(false);
    toast.on_action.take()
}
