//! SaveStates desktop app.
#![forbid(unsafe_code)]

mod app;
mod banner;
mod dialogs;
mod start_screen;
mod tabs;
mod theme;
mod timeline;
mod toast;
mod ui_files;
mod watcher;
mod worker;

/// Marks a translation key that is looked up later rather than right here (by a widget, or
/// chosen from a table), so that the coverage check in savestates-i18n finds it like a `tr`
/// call. Gives the key back unchanged.
pub const fn tr_key(key: &'static str) -> &'static str {
    key
}

fn main() {
    std::process::exit(app::App::run());
}
