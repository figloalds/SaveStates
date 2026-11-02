//! Settings (spec §6.10): each change in the Settings dialog goes through
//! [`settings_flow::apply`], and this carries out what it says.
//!
//! The settings are written only here and by `App::save_settings`, on the UI thread, one write
//! at a time. Looking for Claude Code reads the disk, so it runs on a thread of its own.

use savestates_ai::locate_claude;
use savestates_core::settings::NamingMode;
use savestates_i18n::{Lang, system_language};
use savestates_ui_raven::{locale, task, tr};

use super::{App, run_in_background, with_app};
use crate::dialogs::{inform, settings};
use crate::flows::settings_flow::{self, SettingsChange, SettingsEffect};
use crate::tr_key;

impl App {
    /// The ⚙ button: the Settings dialog.
    pub(super) fn open_settings(&self) {
        let shown = self.settings.borrow().clone();
        settings::show(self.window, &shown, |change| {
            with_app(|app| app.settings_changed(change));
        });
    }

    /// The user changed a setting. Turning on AI naming first looks for Claude Code.
    fn settings_changed(&self, change: SettingsChange) {
        if !matches!(change, SettingsChange::Naming(_)) {
            self.apply_setting(change, false);
            return;
        }
        let asked = self.naming_asked.get() + 1;
        self.naming_asked.set(asked);
        if change != SettingsChange::Naming(NamingMode::Ai) {
            self.apply_setting(change, false);
            return;
        }
        let started = run_in_background(move || {
            let found = locate_claude().is_some();
            task::post(move || with_app(|app| app.claude_looked_for(asked, found)));
        });
        if !started {
            self.apply_setting(change, false);
        }
    }

    /// The search for Claude Code started by the naming choice `asked` is done. A choice made
    /// since then stands.
    fn claude_looked_for(&self, asked: u64, found: bool) {
        if self.naming_asked.get() == asked {
            self.apply_setting(SettingsChange::Naming(NamingMode::Ai), found);
        }
    }

    /// Applies `change`, shows the dialog in line with the result, and carries out the effects.
    fn apply_setting(&self, change: SettingsChange, claude_found: bool) {
        let (next, effects) = {
            let current = self.settings.borrow();
            settings_flow::apply(&current, change, claude_found, &system_language())
        };
        settings::sync(&next);
        *self.settings.borrow_mut() = next;
        for effect in effects {
            match effect {
                SettingsEffect::ShowClaudeMissing => inform(
                    self.window,
                    tr_key("settings.naming.ai"),
                    &tr("ai.claude_missing"),
                    None,
                ),
                SettingsEffect::ShowPrivacyNotice => inform(
                    self.window,
                    tr_key("settings.naming.ai"),
                    &tr("ai.privacy_notice"),
                    None,
                ),
                SettingsEffect::ApplyLanguage(lang) => self.apply_language(lang),
                SettingsEffect::Persist => self.save_settings(),
            }
        }
    }

    /// Shows the app in `lang` from now on: Raven changes the texts of the layouts' `@keys` at
    /// once, and the texts set in code are set again on the next frame (this may run inside
    /// the language list's own callback, and that list's texts are among them).
    fn apply_language(&self, lang: Lang) {
        locale::set_language(lang.code());
        self.lang.set(lang);
        let _ = task::timer(0.0, false, || with_app(|app| app.relabel()));
    }

    /// Sets every text set in code again, in the current language.
    fn relabel(&self) {
        let lang = self.lang.get();
        let pages = self.pages.borrow().clone();
        for tab in pages {
            tab.relabel(lang);
        }
        settings::relabel();
    }
}
