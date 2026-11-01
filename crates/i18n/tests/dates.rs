use chrono::{DateTime, FixedOffset};
use savestates_i18n::{
    Lang, LanguageChoice, relative_date, resolve, startup_failure_text, system_language,
};

fn at(s: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(s).expect("valid timestamp")
}

fn now() -> DateTime<FixedOffset> {
    at("2026-11-01T15:00:00-03:00")
}

fn both(t: &str) -> (String, String) {
    (
        relative_date(Lang::PtBr, at(t), now()),
        relative_date(Lang::En, at(t), now()),
    )
}

#[test]
fn earlier_the_same_day_shows_hoje_and_the_time() {
    assert_eq!(
        both("2026-11-01T10:42:00-03:00"),
        ("hoje 10:42".to_string(), "today 10:42".to_string())
    );
}

#[test]
fn the_time_is_two_digits_each() {
    assert_eq!(
        both("2026-11-01T07:05:00-03:00"),
        ("hoje 07:05".to_string(), "today 07:05".to_string())
    );
}

#[test]
fn the_previous_calendar_day_is_ontem() {
    assert_eq!(
        both("2026-10-31T23:59:00-03:00"),
        ("ontem".to_string(), "yesterday".to_string())
    );
}

#[test]
fn an_earlier_day_this_year_shows_day_and_month() {
    assert_eq!(
        both("2026-10-26T09:00:00-03:00"),
        ("26 de out.".to_string(), "Oct 26".to_string())
    );
}

#[test]
fn another_year_shows_the_year() {
    assert_eq!(
        both("2025-10-26T09:00:00-03:00"),
        ("26 de out. de 2025".to_string(), "Oct 26, 2025".to_string())
    );
}

#[test]
fn t_is_converted_to_the_offset_of_now() {
    // 02:00 UTC on Nov 1 is 23:00 on Oct 31 at -03:00.
    assert_eq!(
        both("2026-11-01T02:00:00Z"),
        ("ontem".to_string(), "yesterday".to_string())
    );
}

#[test]
fn yesterday_is_a_calendar_day_not_24_hours() {
    // 14 hours before now, but still the same local day.
    assert_eq!(
        both("2026-11-01T01:00:00-03:00"),
        ("hoje 01:00".to_string(), "today 01:00".to_string())
    );
    // 15 hours before now, on the previous local day.
    assert_eq!(
        both("2026-10-31T23:59:00-03:00"),
        ("ontem".to_string(), "yesterday".to_string())
    );
}

#[test]
fn every_month_abbreviation_matches_the_spec() {
    let pt = [
        "jan.", "fev.", "mar.", "abr.", "mai.", "jun.", "jul.", "ago.", "set.", "out.", "nov.",
        "dez.",
    ];
    let en = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // `now` is Nov 1, so use days of 2026 that are never today or yesterday.
    for month in 1..=12u32 {
        let t = at(&format!("2026-{month:02}-15T12:00:00-03:00"));
        assert_eq!(
            relative_date(Lang::PtBr, t, now()),
            format!("15 de {}", pt[(month - 1) as usize])
        );
        assert_eq!(
            relative_date(Lang::En, t, now()),
            format!("{} 15", en[(month - 1) as usize])
        );
    }
}

#[test]
fn a_later_day_in_the_same_year_shows_the_date() {
    assert_eq!(
        both("2026-11-20T09:00:00-03:00"),
        ("20 de nov.".to_string(), "Nov 20".to_string())
    );
}

#[test]
fn a_later_time_on_the_same_day_is_still_hoje() {
    assert_eq!(
        both("2026-11-01T15:30:00-03:00"),
        ("hoje 15:30".to_string(), "today 15:30".to_string())
    );
}

#[test]
fn a_later_year_shows_the_year() {
    assert_eq!(
        both("2027-01-05T09:00:00-03:00"),
        ("5 de jan. de 2027".to_string(), "Jan 5, 2027".to_string())
    );
}

#[test]
fn resolve_follows_the_system_for_system() {
    assert_eq!(resolve(LanguageChoice::System, "pt-BR"), Lang::PtBr);
    assert_eq!(resolve(LanguageChoice::System, "pt-PT"), Lang::PtBr);
    assert_eq!(resolve(LanguageChoice::System, "en-US"), Lang::En);
    assert_eq!(resolve(LanguageChoice::System, "fr-FR"), Lang::PtBr);
}

#[test]
fn resolve_honours_an_explicit_choice() {
    assert_eq!(resolve(LanguageChoice::En, "pt-BR"), Lang::En);
    assert_eq!(resolve(LanguageChoice::PtBr, "en-US"), Lang::PtBr);
}

#[test]
fn language_codes_and_ai_names() {
    assert_eq!(Lang::PtBr.code(), "pt-BR");
    assert_eq!(Lang::En.code(), "en");
    assert_eq!(Lang::PtBr.ai_language_name(), "Brazilian Portuguese");
    assert_eq!(Lang::En.ai_language_name(), "English");
}

#[test]
fn startup_failure_text_is_the_agreed_copy() {
    assert_eq!(
        startup_failure_text(Lang::PtBr),
        (
            "SaveStates não conseguiu abrir",
            "Não foi possível iniciar os gráficos (Vulkan). Atualize o driver da sua placa de vídeo e tente de novo."
        )
    );
    assert_eq!(
        startup_failure_text(Lang::En),
        (
            "SaveStates couldn't start",
            "The graphics system (Vulkan) couldn't start. Update your graphics driver and try again."
        )
    );
}

#[test]
fn system_language_is_never_empty() {
    assert!(!system_language().is_empty());
}

#[test]
fn language_choice_defaults_to_system() {
    assert_eq!(LanguageChoice::default(), LanguageChoice::System);
}
