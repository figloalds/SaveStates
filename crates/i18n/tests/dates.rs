use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime};
use savestates_i18n::{
    Lang, LanguageChoice, day_label, day_label_full, hour_label, minute_label, month_label,
    month_name, relative_date, relative_date_local, resolve, short_date, startup_failure_text,
    system_language, time_label, week_start_label,
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
fn local_times_are_labeled_as_they_are() {
    let local = |s: &str| s.parse::<NaiveDateTime>().expect("valid local time");
    let now = local("2026-11-01T15:00:00");
    let label = |lang: Lang, t: &str| relative_date_local(lang, local(t), now);
    assert_eq!(label(Lang::PtBr, "2026-11-01T10:42:00"), "hoje 10:42");
    assert_eq!(label(Lang::En, "2026-11-01T10:42:00"), "today 10:42");
    assert_eq!(label(Lang::PtBr, "2026-10-31T23:59:00"), "ontem");
    assert_eq!(label(Lang::PtBr, "2026-10-26T08:00:00"), "26 de out.");
    assert_eq!(label(Lang::En, "2025-10-26T08:00:00"), "Oct 26, 2025");
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

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

#[test]
fn day_label_for_today_is_hoje_or_today() {
    let today = day(2026, 11, 1);
    for with_month in [false, true] {
        assert_eq!(day_label_full(Lang::PtBr, today, today, with_month), "hoje");
        assert_eq!(day_label_full(Lang::En, today, today, with_month), "today");
    }
    assert_eq!(day_label(Lang::PtBr, today, today), "hoje");
    assert_eq!(day_label(Lang::En, today, today), "today");
}

#[test]
fn day_label_full_with_the_month_has_weekday_day_and_month() {
    let today = day(2026, 11, 1);
    // 2025-10-26 is a Sunday.
    assert_eq!(
        day_label_full(Lang::PtBr, day(2025, 10, 26), today, true),
        "dom 26 out"
    );
    assert_eq!(
        day_label_full(Lang::En, day(2025, 10, 26), today, true),
        "Sun Oct 26"
    );
    // 2025-10-29 is a Wednesday, not the first of a month.
    assert_eq!(
        day_label_full(Lang::PtBr, day(2025, 10, 29), today, true),
        "qua 29 out"
    );
    assert_eq!(
        day_label_full(Lang::En, day(2025, 10, 29), today, true),
        "Wed Oct 29"
    );
}

#[test]
fn day_label_full_without_the_month_has_weekday_and_day() {
    let today = day(2026, 11, 1);
    // 2025-10-27 is a Monday.
    assert_eq!(
        day_label_full(Lang::PtBr, day(2025, 10, 27), today, false),
        "seg 27"
    );
    assert_eq!(
        day_label_full(Lang::En, day(2025, 10, 27), today, false),
        "Mon 27"
    );
}

#[test]
fn day_label_leaves_the_month_off() {
    let today = day(2026, 11, 1);
    assert_eq!(day_label(Lang::PtBr, day(2025, 10, 27), today), "seg 27");
    assert_eq!(day_label(Lang::En, day(2025, 10, 27), today), "Mon 27");
}

#[test]
fn the_first_of_a_month_always_shows_its_month() {
    // 2026-11-01 is a Sunday; with `today` elsewhere it is an ordinary day.
    let today = day(2026, 11, 15);
    assert_eq!(
        day_label_full(Lang::PtBr, day(2026, 11, 1), today, false),
        "dom 1 nov"
    );
    assert_eq!(
        day_label_full(Lang::En, day(2026, 11, 1), today, false),
        "Sun Nov 1"
    );
    assert_eq!(day_label(Lang::PtBr, day(2026, 11, 1), today), "dom 1 nov");
}

#[test]
fn every_weekday_abbreviation_matches_the_spec() {
    let pt = ["seg", "ter", "qua", "qui", "sex", "sáb", "dom"];
    let en = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let today = day(2030, 1, 1);
    // 2026-10-05 is a Monday, and the next six days follow it.
    for offset in 0..7u32 {
        let date = day(2026, 10, 5 + offset);
        assert_eq!(
            day_label_full(Lang::PtBr, date, today, false),
            format!("{} {}", pt[offset as usize], 5 + offset)
        );
        assert_eq!(
            day_label_full(Lang::En, date, today, false),
            format!("{} {}", en[offset as usize], 5 + offset)
        );
    }
}

#[test]
fn hour_label_is_24_hour_in_both_languages() {
    assert_eq!(hour_label(Lang::PtBr, 13), "13h");
    assert_eq!(hour_label(Lang::PtBr, 0), "0h");
    assert_eq!(hour_label(Lang::PtBr, 5), "5h");
    // English writes hours as the minute ticks and the saves' times do.
    assert_eq!(hour_label(Lang::En, 13), "13:00");
    assert_eq!(hour_label(Lang::En, 0), "00:00");
    assert_eq!(hour_label(Lang::En, 1), "01:00");
    assert_eq!(hour_label(Lang::En, 12), "12:00");
    assert_eq!(hour_label(Lang::En, 23), "23:00");
    assert_eq!(hour_label(Lang::En, 24), "00:00");
}

#[test]
fn minute_and_time_labels_are_two_digit_24_hour() {
    assert_eq!(minute_label(13, 5), "13:05");
    assert_eq!(minute_label(0, 0), "00:00");
    let t = NaiveTime::from_hms_opt(15, 2, 59).expect("valid time");
    assert_eq!(time_label(t), "15:02");
}

#[test]
fn short_date_reuses_the_month_abbreviations() {
    assert_eq!(short_date(Lang::PtBr, day(2026, 10, 31)), "31 de out.");
    assert_eq!(short_date(Lang::En, day(2026, 10, 31)), "Oct 31");
    assert_eq!(short_date(Lang::PtBr, day(2026, 1, 5)), "5 de jan.");
    assert_eq!(short_date(Lang::En, day(2026, 1, 5)), "Jan 5");
}

#[test]
fn week_start_label_is_day_and_month_without_a_period() {
    // 2026-09-28 is a Monday.
    assert_eq!(week_start_label(Lang::PtBr, day(2026, 9, 28)), "28 set");
    assert_eq!(week_start_label(Lang::En, day(2026, 9, 28)), "Sep 28");
}

#[test]
fn month_label_and_month_name() {
    assert_eq!(month_label(Lang::PtBr, 2026, 10), "out 2026");
    assert_eq!(month_label(Lang::En, 2026, 10), "Oct 2026");
    assert_eq!(month_name(Lang::PtBr, 10), "outubro");
    assert_eq!(month_name(Lang::En, 10), "October");
}

#[test]
fn every_month_name_and_label_matches_the_spec() {
    let pt = [
        "janeiro",
        "fevereiro",
        "março",
        "abril",
        "maio",
        "junho",
        "julho",
        "agosto",
        "setembro",
        "outubro",
        "novembro",
        "dezembro",
    ];
    let pt_short = [
        "jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez",
    ];
    let en = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let en_short = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    for month in 1..=12u32 {
        let i = (month - 1) as usize;
        assert_eq!(month_name(Lang::PtBr, month), pt[i]);
        assert_eq!(month_name(Lang::En, month), en[i]);
        assert_eq!(
            month_label(Lang::PtBr, 2027, month),
            format!("{} 2027", pt_short[i])
        );
        assert_eq!(
            month_label(Lang::En, 2027, month),
            format!("{} 2027", en_short[i])
        );
    }
}

#[test]
fn a_month_outside_1_to_12_does_not_panic() {
    assert_eq!(month_name(Lang::En, 0), "January");
    assert_eq!(month_name(Lang::En, 13), "December");
    assert_eq!(month_label(Lang::PtBr, 2026, 99), "dez 2026");
}
