//! PT-BR/EN translation table, language resolution and relative dates.
//!
//! Every user-facing string lives in `translations.csv`, which the UI loads at runtime.
//! PT-BR is the first column and the fallback language.
#![forbid(unsafe_code)]

mod dates;

use serde::{Deserialize, Serialize};

pub use dates::{
    day_label, day_label_full, hour_label, minute_label, month_label, month_name, relative_date,
    relative_date_local, short_date, time_label, week_start_label,
};

/// The translation table: `key,pt-BR,en`, one row per key.
pub const TRANSLATIONS_CSV: &str = include_str!("../translations.csv");

/// A language the interface is available in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    PtBr,
    En,
}

impl Lang {
    /// The column header of this language in the translation table.
    pub fn code(self) -> &'static str {
        match self {
            Lang::PtBr => "pt-BR",
            Lang::En => "en",
        }
    }

    /// The language's name as written in an instruction to the AI.
    pub fn ai_language_name(self) -> &'static str {
        match self {
            Lang::PtBr => "Brazilian Portuguese",
            Lang::En => "English",
        }
    }
}

/// The language setting a person picks: follow the system, or force one language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LanguageChoice {
    #[default]
    System,
    PtBr,
    En,
}

/// The language to show: the explicit choice, or the one `system_code` (like `en-US`) asks for.
/// Anything that is neither Portuguese nor English falls back to PT-BR.
pub fn resolve(choice: LanguageChoice, system_code: &str) -> Lang {
    match choice {
        LanguageChoice::PtBr => Lang::PtBr,
        LanguageChoice::En => Lang::En,
        LanguageChoice::System => {
            let code = system_code.trim().to_ascii_lowercase();
            if code.starts_with("en") {
                Lang::En
            } else {
                Lang::PtBr
            }
        }
    }
}

/// The system's locale code, like `pt-BR` or `en-US`. Falls back to `pt-BR`.
pub fn system_language() -> String {
    sys_locale::get_locale()
        .filter(|code| !code.trim().is_empty())
        .unwrap_or_else(|| "pt-BR".to_string())
}

/// Title and message for the dialog shown when the graphics system fails to start.
///
/// The one string pair kept outside the translation table: the UI library that loads the
/// table is not running yet when this is needed.
pub fn startup_failure_text(lang: Lang) -> (&'static str, &'static str) {
    match lang {
        Lang::PtBr => (
            "SaveStates não conseguiu abrir",
            "Não foi possível iniciar os gráficos (Vulkan). Atualize o driver da sua placa de vídeo e tente de novo.",
        ),
        Lang::En => (
            "SaveStates couldn't start",
            "The graphics system (Vulkan) couldn't start. Update your graphics driver and try again.",
        ),
    }
}
