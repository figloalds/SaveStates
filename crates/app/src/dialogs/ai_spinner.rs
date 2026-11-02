//! While the AI thinks of a name (spec §6.9): a bar that keeps moving, and Cancel. The user can
//! always cancel and type a name instead.

use std::cell::{Cell, RefCell};
use std::time::Instant;

use savestates_ui_raven::{Timer, Widget, Window, bind_action, dialog, task};

use crate::dialogs::{after_close, build, part};

const FILE: &str = "ai_spinner.xml";
/// The dialog's result when [`hide`] closed it (the user's Cancel and Escape give 0).
const HIDDEN: i32 = 1;
/// How often the bar moves, in seconds.
const TICK: f64 = 1.0 / 30.0;
/// Seconds for the bar to fill (then it empties in as long).
const SWEEP: f64 = 0.9;

/// The dialog while it is open.
struct Open {
    content: Widget,
    timer: Timer,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static BOUND: Cell<bool> = const { Cell::new(false) };
}

/// Shows that the AI is thinking, in `window`. `on_cancel` runs when the user cancels (Cancel or
/// Escape), not when [`hide`] closes it.
pub fn show(window: Window, on_cancel: impl FnOnce() + 'static) {
    bind_actions();
    hide();
    let parts = build(FILE).and_then(|content| Ok((content, part(content, FILE, "ai.progress")?)));
    let (content, bar) = match parts {
        Ok(parts) => parts,
        Err(error) => {
            // Without the spinner the AI still answers; the user just sees no wait.
            eprintln!("the AI spinner could not be built: {error}");
            return;
        }
    };
    let started = Instant::now();
    let timer = task::timer(TICK, true, move || {
        bar.set_progress(pulse(started.elapsed().as_secs_f64()));
    });
    OPEN.set(Some(Open { content, timer }));
    dialog::open(window, content, move |result| {
        if let Some(open) = OPEN.take() {
            open.timer.cancel();
        }
        if result != HIDDEN {
            after_close(on_cancel);
        }
    });
}

/// Closes the spinner, if it is showing.
pub fn hide() {
    let content = OPEN.with_borrow(|open| open.as_ref().map(|open| open.content));
    if let Some(content) = content {
        dialog::close(content, HIDDEN);
    }
}

/// Binds the dialog's action, once.
fn bind_actions() {
    if BOUND.replace(true) {
        return;
    }
    bind_action("ai.cancel", |sender| dialog::close(sender, 0));
}

/// How full the bar is `seconds` after it appeared: it fills and empties, over and over.
fn pulse(seconds: f64) -> f32 {
    let phase = (seconds / SWEEP).rem_euclid(2.0);
    let fill = if phase <= 1.0 { phase } else { 2.0 - phase };
    fill as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_fills_and_empties() {
        assert_eq!(pulse(0.0), 0.0);
        assert!((pulse(SWEEP / 2.0) - 0.5).abs() < 1e-6);
        assert!((pulse(SWEEP) - 1.0).abs() < 1e-6);
        assert!((pulse(SWEEP * 1.5) - 0.5).abs() < 1e-6);
        assert!(pulse(SWEEP * 2.0).abs() < 1e-6);
        assert!((pulse(SWEEP * 7.25) - 0.75).abs() < 1e-6);
    }
}
