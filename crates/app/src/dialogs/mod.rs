//! Dialogs shown inside the main window.
//!
//! Raven deletes a dialog's content when it closes, so each dialog builds its content again
//! every time it opens, and its buttons fire actions bound once by name. An opening leaks only
//! the few bytes of the callbacks Raven never frees: the closed callback's slot and, for the
//! dialogs with text fields or a moving bar, their change and timer callbacks.

pub mod ai_spinner;
pub mod archived;
pub mod confirm;
pub mod identity;
pub mod name;
pub mod recovery;
pub mod rename;
pub mod settings;
pub mod warnings;

pub use confirm::{confirm, confirm_destructive, inform};

use savestates_ui_raven::{Widget, task, widget_from_xml};

use crate::ui_files;

/// Runs `f` on the next frame. A dialog's closed callback runs while Raven is still closing
/// it, and a dialog opened from there never shows: what an answer leads to, which may be
/// another dialog, runs once the closing is over.
fn after_close(f: impl FnOnce() + 'static) {
    let mut f = Some(f);
    let _ = task::timer(0.0, false, move || {
        if let Some(f) = f.take() {
            f();
        }
    });
}

/// A new copy of a dialog's content, from `crates/app/ui/<file>`.
fn build(file: &str) -> Result<Widget, String> {
    widget_from_xml(&ui_files::source(file))
}

/// The widget named `name` in `content`, built from `file`.
fn part(content: Widget, file: &str, name: &str) -> Result<Widget, String> {
    content
        .find(name)
        .ok_or_else(|| format!("{file} has no widget named {name}"))
}
