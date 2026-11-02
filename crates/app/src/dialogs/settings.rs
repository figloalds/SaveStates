//! The Settings dialog (spec §6.10), opened by the ⚙ button: how saves get their names and the
//! language, then, folded away, the AI model, how long the AI may take and from what size a
//! file is large.
//!
//! Each change goes to the app at once; the app answers by putting the dialog in line with the
//! settings it ends up with ([`sync`]): turning on AI naming without Claude Code, say, puts
//! "Ask me" back. The model is taken on Enter, or when the dialog closes, rather than at every
//! key. Like every dialog here, its content is built again each time it opens (Raven deletes
//! it on close) and its controls fire actions bound once.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use savestates_core::settings::{NamingMode, Settings};
use savestates_i18n::LanguageChoice;
use savestates_ui_raven::{Widget, Window, bind_action, combo, dialog, tr};

use crate::dialogs::{after_close, build, part};
use crate::flows::settings_flow::SettingsChange;

const FILE: &str = "dialog_settings.xml";

/// The dialog while it is open.
struct Open {
    naming: Widget,
    language: Widget,
    model: Widget,
    timeout: Widget,
    large_file: Widget,
    /// The settings the controls show.
    shown: Settings,
    /// Where the user's changes go.
    on_change: Rc<dyn Fn(SettingsChange)>,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static BOUND: Cell<bool> = const { Cell::new(false) };
}

/// Shows the settings `s` in `window`. `on_change` gets each change the user makes; answer it
/// with [`sync`].
pub fn show(window: Window, s: &Settings, on_change: impl Fn(SettingsChange) + 'static) {
    bind_actions();
    let parts = build(FILE).and_then(|content| {
        let find = |name| part(content, FILE, name);
        Ok((
            content,
            Open {
                naming: find("settings.naming")?,
                language: find("settings.language")?,
                model: find("settings.ai_model")?,
                timeout: find("settings.ai_timeout")?,
                large_file: find("settings.large_file_mb")?,
                shown: s.clone(),
                on_change: Rc::new(on_change),
            },
        ))
    });
    let (content, open) = match parts {
        Ok(parts) => parts,
        Err(error) => {
            eprintln!("the settings dialog could not be built: {error}");
            return;
        }
    };
    fill_choices(&open);
    open.model.text_input_set(&s.ai_model);
    show_numbers(&open);
    OPEN.set(Some(open));
    dialog::open(window, content, |_| {
        // The model typed without Enter is taken now.
        let Some(open) = OPEN.take() else { return };
        let typed = open.model.text_input_get();
        if typed.trim() != open.shown.ai_model {
            let on_change = open.on_change;
            after_close(move || on_change(SettingsChange::AiModel(typed)));
        }
    });
}

/// Puts the controls in line with the settings `s` (after the app applied a change, or turned
/// it down). The model the user may be typing is left alone.
pub fn sync(s: &Settings) {
    OPEN.with_borrow_mut(|open| {
        let Some(open) = open.as_mut() else { return };
        open.shown = s.clone();
        select(open.naming, naming_index(s.naming));
        select(open.language, language_index(s.language));
        show_numbers(open);
    });
}

/// The language changed: shows the choices in it.
pub fn relabel() {
    OPEN.with_borrow(|open| {
        if let Some(open) = open {
            fill_choices(open);
        }
    });
}

/// Binds the dialog's actions, once.
fn bind_actions() {
    if BOUND.replace(true) {
        return;
    }
    // Always reported, even when it matches the settings: "Ask me" chosen while Claude Code is
    // still being looked for has to win over what that search finds.
    bind_action("settings.naming", |_| {
        report(|open| {
            let naming = naming_at(combo::selected(open.naming)?);
            Some(SettingsChange::Naming(naming))
        });
    });
    bind_action("settings.language", |_| {
        report(|open| {
            let language = language_at(combo::selected(open.language)?)?;
            (language != open.shown.language).then_some(SettingsChange::Language(language))
        });
    });
    // Enter in the model field: it is taken, then shown as kept (an empty one is the default).
    bind_action("settings.ai_model", |_| {
        report(|open| {
            let typed = open.model.text_input_get();
            (typed.trim() != open.shown.ai_model).then_some(SettingsChange::AiModel(typed))
        });
        OPEN.with_borrow(|open| {
            if let Some(open) = open {
                open.model.text_input_set(&open.shown.ai_model);
            }
        });
    });
    bind_action("settings.ai_timeout", |_| {
        report(|open| {
            let secs = whole(open.timeout.spin_box_value());
            (secs != open.shown.ai_timeout_secs).then_some(SettingsChange::AiTimeout(secs))
        });
    });
    bind_action("settings.large_file_mb", |_| {
        report(|open| {
            let mb = whole(open.large_file.spin_box_value());
            (mb != open.shown.large_file_mb).then_some(SettingsChange::LargeFileMb(mb))
        });
    });
    bind_action("settings.close", |sender| dialog::close(sender, 0));
}

/// Gives the app the change `change` finds in the dialog, if any. The dialog is not borrowed
/// meanwhile: the app answers with [`sync`].
fn report(change: impl FnOnce(&Open) -> Option<SettingsChange>) {
    let found = OPEN.with_borrow(|open| {
        let open = open.as_ref()?;
        Some((change(open)?, open.on_change.clone()))
    });
    if let Some((change, on_change)) = found {
        on_change(change);
    }
}

/// The choices of both lists, in the current language, with the shown ones selected.
fn fill_choices(open: &Open) {
    let naming = [tr("settings.naming.ask"), tr("settings.naming.ai")];
    let language = [
        tr("settings.language.system"),
        tr("settings.language.pt_br"),
        tr("settings.language.en"),
    ];
    for (list, items, selected) in [
        (open.naming, &naming[..], naming_index(open.shown.naming)),
        (
            open.language,
            &language[..],
            language_index(open.shown.language),
        ),
    ] {
        let items: Vec<&str> = items.iter().map(String::as_str).collect();
        combo::set_items(list, &items);
        combo::set_selected(list, selected);
    }
}

/// The AI timeout and the large-file size, as shown.
fn show_numbers(open: &Open) {
    for (spin, value) in [
        (open.timeout, open.shown.ai_timeout_secs),
        (open.large_file, open.shown.large_file_mb),
    ] {
        if whole(spin.spin_box_value()) != value {
            spin.spin_box_set_value(value as f64);
        }
    }
}

/// Selects item `i` of a list, unless it is selected already.
fn select(list: Widget, i: usize) {
    if combo::selected(list) != Some(i) {
        combo::set_selected(list, i);
    }
}

/// The place of each naming mode in its list.
fn naming_index(naming: NamingMode) -> usize {
    match naming {
        NamingMode::AskMe => 0,
        NamingMode::Ai => 1,
    }
}

fn naming_at(i: usize) -> NamingMode {
    if i == 1 {
        NamingMode::Ai
    } else {
        NamingMode::AskMe
    }
}

/// The place of each language choice in its list.
fn language_index(language: LanguageChoice) -> usize {
    match language {
        LanguageChoice::System => 0,
        LanguageChoice::PtBr => 1,
        LanguageChoice::En => 2,
    }
}

fn language_at(i: usize) -> Option<LanguageChoice> {
    [
        LanguageChoice::System,
        LanguageChoice::PtBr,
        LanguageChoice::En,
    ]
    .get(i)
    .copied()
}

/// A spin box's value as a whole number (rounded; nothing below 0).
fn whole(value: f64) -> u64 {
    // A float-to-int `as` saturates, and NaN gives 0.
    value.round().max(0.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_choice_has_its_place_in_the_lists() {
        for naming in [NamingMode::AskMe, NamingMode::Ai] {
            assert_eq!(naming_at(naming_index(naming)), naming);
        }
        assert_eq!(naming_index(NamingMode::AskMe), 0);
        for language in [
            LanguageChoice::System,
            LanguageChoice::PtBr,
            LanguageChoice::En,
        ] {
            assert_eq!(language_at(language_index(language)), Some(language));
        }
        assert_eq!(language_index(LanguageChoice::System), 0);
        assert_eq!(language_at(3), None);
    }

    #[test]
    fn spin_box_values_are_whole_numbers() {
        assert_eq!(whole(30.0), 30);
        assert_eq!(whole(29.6), 30);
        assert_eq!(whole(-4.0), 0);
        assert_eq!(whole(f64::NAN), 0);
        assert_eq!(whole(1e30), u64::MAX);
    }
}
