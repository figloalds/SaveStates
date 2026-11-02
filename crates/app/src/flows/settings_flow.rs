//! A change made in the Settings dialog (spec §6.10): the settings it leads to, and what the app
//! does about it.
//!
//! Turning on AI naming needs Claude Code (spec §6.9): without it, "Ask me" stays and the user
//! is told why. The first time it is on, a notice says that the changes go to Anthropic.

use savestates_core::settings::{NamingMode, Settings};
use savestates_i18n::{Lang, LanguageChoice, resolve};

/// The shortest and longest the AI may take, in seconds.
const AI_TIMEOUT_SECS: (u64, u64) = (5, 120);
/// The smallest and largest "large file" size, in MB.
const LARGE_FILE_MB: (u64, u64) = (1, 2048);

/// What the user changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsChange {
    Naming(NamingMode),
    Language(LanguageChoice),
    AiModel(String),
    AiTimeout(u64),
    LargeFileMb(u64),
}

/// What the app does after a change, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsEffect {
    /// Claude Code is not on this computer: say so ("Ask me" stays).
    ShowClaudeMissing,
    /// The one-time notice that, with AI naming, the changes go to Anthropic.
    ShowPrivacyNotice,
    /// Show the app in this language now.
    ApplyLanguage(Lang),
    /// Write the settings file.
    Persist,
}

/// The settings after `change` to `s`, and what to do about it. `claude_found` says whether
/// Claude Code was found (looked for when AI naming is turned on); `system_lang` is the
/// system's language code, for "System".
pub fn apply(
    s: &Settings,
    change: SettingsChange,
    claude_found: bool,
    system_lang: &str,
) -> (Settings, Vec<SettingsEffect>) {
    let mut next = s.clone();
    let effects = match change {
        SettingsChange::Naming(NamingMode::Ai) if !claude_found => {
            next.naming = NamingMode::AskMe;
            vec![SettingsEffect::ShowClaudeMissing]
        }
        SettingsChange::Naming(NamingMode::Ai) if !s.ai_notice_shown => {
            next.naming = NamingMode::Ai;
            next.ai_notice_shown = true;
            vec![SettingsEffect::ShowPrivacyNotice, SettingsEffect::Persist]
        }
        SettingsChange::Naming(naming) => {
            next.naming = naming;
            vec![SettingsEffect::Persist]
        }
        SettingsChange::Language(choice) => {
            next.language = choice;
            vec![
                SettingsEffect::ApplyLanguage(resolve(choice, system_lang)),
                SettingsEffect::Persist,
            ]
        }
        SettingsChange::AiModel(model) => {
            let model = model.trim();
            next.ai_model = if model.is_empty() {
                Settings::default().ai_model
            } else {
                model.to_owned()
            };
            vec![SettingsEffect::Persist]
        }
        SettingsChange::AiTimeout(secs) => {
            next.ai_timeout_secs = secs.clamp(AI_TIMEOUT_SECS.0, AI_TIMEOUT_SECS.1);
            vec![SettingsEffect::Persist]
        }
        SettingsChange::LargeFileMb(mb) => {
            next.large_file_mb = mb.clamp(LARGE_FILE_MB.0, LARGE_FILE_MB.1);
            vec![SettingsEffect::Persist]
        }
    };
    (next, effects)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice_shown() -> Settings {
        Settings {
            ai_notice_shown: true,
            ..Settings::default()
        }
    }

    #[test]
    fn ai_without_claude_stays_on_ask_me_and_says_why() {
        let (s, effects) = apply(
            &Settings::default(),
            SettingsChange::Naming(NamingMode::Ai),
            false,
            "pt-BR",
        );
        assert_eq!(s.naming, NamingMode::AskMe);
        assert!(!s.ai_notice_shown);
        assert_eq!(effects, [SettingsEffect::ShowClaudeMissing]);
    }

    #[test]
    fn ai_the_first_time_shows_the_privacy_notice() {
        let (s, effects) = apply(
            &Settings::default(),
            SettingsChange::Naming(NamingMode::Ai),
            true,
            "pt-BR",
        );
        assert_eq!(s.naming, NamingMode::Ai);
        assert!(s.ai_notice_shown);
        assert_eq!(
            effects,
            [SettingsEffect::ShowPrivacyNotice, SettingsEffect::Persist]
        );
    }

    #[test]
    fn ai_again_does_not_show_the_notice() {
        let (s, effects) = apply(
            &notice_shown(),
            SettingsChange::Naming(NamingMode::Ai),
            true,
            "pt-BR",
        );
        assert_eq!(s.naming, NamingMode::Ai);
        assert_eq!(effects, [SettingsEffect::Persist]);
    }

    #[test]
    fn a_language_is_applied_at_once_and_kept() {
        let (s, effects) = apply(
            &Settings::default(),
            SettingsChange::Language(LanguageChoice::En),
            false,
            "pt-BR",
        );
        assert_eq!(s.language, LanguageChoice::En);
        assert_eq!(
            effects,
            [
                SettingsEffect::ApplyLanguage(Lang::En),
                SettingsEffect::Persist
            ]
        );
        // "System" follows the system's language.
        let (s, effects) = apply(
            &s,
            SettingsChange::Language(LanguageChoice::System),
            false,
            "en-US",
        );
        assert_eq!(s.language, LanguageChoice::System);
        assert_eq!(effects[0], SettingsEffect::ApplyLanguage(Lang::En));
        let (_, effects) = apply(
            &s,
            SettingsChange::Language(LanguageChoice::System),
            false,
            "de-DE",
        );
        assert_eq!(effects[0], SettingsEffect::ApplyLanguage(Lang::PtBr));
    }

    #[test]
    fn numbers_are_kept_within_their_range() {
        let s = Settings::default();
        let (t, effects) = apply(&s, SettingsChange::AiTimeout(1), false, "pt-BR");
        assert_eq!(t.ai_timeout_secs, 5);
        assert_eq!(effects, [SettingsEffect::Persist]);
        let (t, _) = apply(&s, SettingsChange::AiTimeout(500), false, "pt-BR");
        assert_eq!(t.ai_timeout_secs, 120);
        let (t, _) = apply(&s, SettingsChange::AiTimeout(45), false, "pt-BR");
        assert_eq!(t.ai_timeout_secs, 45);

        let (l, effects) = apply(&s, SettingsChange::LargeFileMb(0), false, "pt-BR");
        assert_eq!(l.large_file_mb, 1);
        assert_eq!(effects, [SettingsEffect::Persist]);
        let (l, _) = apply(&s, SettingsChange::LargeFileMb(10_000), false, "pt-BR");
        assert_eq!(l.large_file_mb, 2048);
        let (l, _) = apply(&s, SettingsChange::LargeFileMb(100), false, "pt-BR");
        assert_eq!(l.large_file_mb, 100);
    }

    #[test]
    fn ask_me_and_the_model_are_kept() {
        let ai = Settings {
            naming: NamingMode::Ai,
            ..notice_shown()
        };
        let (s, effects) = apply(
            &ai,
            SettingsChange::Naming(NamingMode::AskMe),
            false,
            "pt-BR",
        );
        assert_eq!(s.naming, NamingMode::AskMe);
        assert!(s.ai_notice_shown, "the notice was shown once, for good");
        assert_eq!(effects, [SettingsEffect::Persist]);

        let (s, effects) = apply(
            &ai,
            SettingsChange::AiModel("  sonnet ".to_owned()),
            false,
            "pt-BR",
        );
        assert_eq!(s.ai_model, "sonnet");
        assert_eq!(effects, [SettingsEffect::Persist]);
        // An empty model means the default one.
        let (s, _) = apply(
            &s,
            SettingsChange::AiModel("   ".to_owned()),
            false,
            "pt-BR",
        );
        assert_eq!(s.ai_model, "haiku");
    }

    #[test]
    fn only_the_change_is_applied() {
        let before = Settings {
            ai_model: "opus".to_owned(),
            large_file_mb: 50,
            ..notice_shown()
        };
        let (after, _) = apply(&before, SettingsChange::AiTimeout(60), false, "pt-BR");
        assert_eq!(
            after,
            Settings {
                ai_timeout_secs: 60,
                ..before
            }
        );
    }
}
