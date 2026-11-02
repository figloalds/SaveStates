//! SaveStates desktop app.
#![forbid(unsafe_code)]
// A release build is a GUI program: no console window opens behind it.
#![cfg_attr(all(not(debug_assertions), not(test)), windows_subsystem = "windows")]

// First, so that every module after it can use `log!`.
#[macro_use]
mod log;

mod app;
mod banner;
mod bring_back_ui;
mod changes_panel;
mod dialogs;
mod diffpane;
mod files_tree;
mod flows;
mod peek;
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
