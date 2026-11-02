//! Timeline date labels: "hoje 10:42", "ontem", "26 de out.", "26 de out. de 2025", and the
//! fragments the day grid draws: "dom 26 out", "13h" (EN "13:00"), "13:05", "semana de 28 set",
//! "out 2026".

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

use crate::Lang;

const MONTHS_PT_BR: [&str; 12] = [
    "jan.", "fev.", "mar.", "abr.", "mai.", "jun.", "jul.", "ago.", "set.", "out.", "nov.", "dez.",
];

const MONTHS_EN: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const MONTH_NAMES_PT_BR: [&str; 12] = [
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

const MONTH_NAMES_EN: [&str; 12] = [
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

/// Weekdays counted from Sunday.
const WEEKDAYS_PT_BR: [&str; 7] = ["dom", "seg", "ter", "qua", "qui", "sex", "sáb"];
const WEEKDAYS_EN: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Zero-based index of a month number. A month outside 1 to 12 is clamped to the nearest one,
/// so a bad number never panics.
fn month_index(month: u32) -> usize {
    (month.clamp(1, 12) - 1) as usize
}

/// The month abbreviation without a trailing period: "out" for PT-BR, "Oct" for EN.
fn month_abbr(lang: Lang, month0: usize) -> &'static str {
    match lang {
        Lang::PtBr => MONTHS_PT_BR[month0].trim_end_matches('.'),
        Lang::En => MONTHS_EN[month0],
    }
}

/// Label for the moment `t`, as seen from `now`.
///
/// `t` is first moved to the offset of `now`, so "today" and "yesterday" are calendar days
/// in `now`'s offset. A `t` in the future (clock skew) follows the same rules: a later day
/// in the same year shows the date.
pub fn relative_date(lang: Lang, t: DateTime<FixedOffset>, now: DateTime<FixedOffset>) -> String {
    let t = t.with_timezone(&now.timezone());
    relative_date_local(lang, t.naive_local(), now.naive_local())
}

/// [`relative_date`] for two local wall-clock times: `t` as seen from `now`, both read in the
/// same time zone.
pub fn relative_date_local(lang: Lang, t: NaiveDateTime, now: NaiveDateTime) -> String {
    let day = t.date();
    let today = now.date();

    if day == today {
        let time = time_label(t.time());
        return match lang {
            Lang::PtBr => format!("hoje {time}"),
            Lang::En => format!("today {time}"),
        };
    }
    if today.pred_opt() == Some(day) {
        return match lang {
            Lang::PtBr => "ontem".to_string(),
            Lang::En => "yesterday".to_string(),
        };
    }

    if t.year() == now.year() {
        return short_date(lang, day);
    }
    let month = t.month0() as usize;
    match lang {
        Lang::PtBr => format!("{} de {} de {}", t.day(), MONTHS_PT_BR[month], t.year()),
        Lang::En => format!("{} {}, {}", MONTHS_EN[month], t.day(), t.year()),
    }
}

/// Header of a day column without the month: "hoje" for `today`, otherwise the weekday and the
/// day ("seg 27"). The first of a month still shows its month ("dom 1 nov").
/// `day_label_full` can add the month to any day.
pub fn day_label(lang: Lang, date: NaiveDate, today: NaiveDate) -> String {
    day_label_full(lang, date, today, false)
}

/// Header of a day column. "hoje" / "today" for `today`; otherwise the weekday and the day,
/// with the month added when `with_month` is set and always on the first of a month:
/// PT-BR "dom 26 out" / "seg 27", EN "Sun Oct 26" / "Mon 27".
pub fn day_label_full(lang: Lang, date: NaiveDate, today: NaiveDate, with_month: bool) -> String {
    if date == today {
        return match lang {
            Lang::PtBr => "hoje".to_string(),
            Lang::En => "today".to_string(),
        };
    }
    let weekday = date.weekday().num_days_from_sunday() as usize;
    let day = date.day();
    let month = month_abbr(lang, date.month0() as usize);
    let with_month = with_month || day == 1;
    match (lang, with_month) {
        (Lang::PtBr, true) => format!("{} {day} {month}", WEEKDAYS_PT_BR[weekday]),
        (Lang::PtBr, false) => format!("{} {day}", WEEKDAYS_PT_BR[weekday]),
        (Lang::En, true) => format!("{} {month} {day}", WEEKDAYS_EN[weekday]),
        (Lang::En, false) => format!("{} {day}", WEEKDAYS_EN[weekday]),
    }
}

/// An hour tick on the 24-hour clock: PT-BR "13h", EN "13:00" (as its minute ticks and the
/// saves' times are written).
pub fn hour_label(lang: Lang, hour: u32) -> String {
    let hour = hour % 24;
    match lang {
        Lang::PtBr => format!("{hour}h"),
        Lang::En => minute_label(hour, 0),
    }
}

/// A minute tick: "13:05" in both languages.
pub fn minute_label(hour: u32, minute: u32) -> String {
    format!("{hour:02}:{minute:02}")
}

/// A time of day: "15:02" in both languages.
pub fn time_label(t: NaiveTime) -> String {
    minute_label(t.hour(), t.minute())
}

/// A short date for tooltips: PT-BR "31 de out.", EN "Oct 31".
pub fn short_date(lang: Lang, date: NaiveDate) -> String {
    let month = date.month0() as usize;
    match lang {
        Lang::PtBr => format!("{} de {}", date.day(), MONTHS_PT_BR[month]),
        Lang::En => format!("{} {}", MONTHS_EN[month], date.day()),
    }
}

/// The start of a week as the grid prints it: PT-BR "28 set", EN "Sep 28".
/// Weeks start on Monday, so the caller passes that Monday.
pub fn week_start_label(lang: Lang, monday: NaiveDate) -> String {
    let month = month_abbr(lang, monday.month0() as usize);
    match lang {
        Lang::PtBr => format!("{} {month}", monday.day()),
        Lang::En => format!("{month} {}", monday.day()),
    }
}

/// A month in the grid header: PT-BR "out 2026", EN "Oct 2026". `month` is 1 to 12.
pub fn month_label(lang: Lang, year: i32, month: u32) -> String {
    format!("{} {year}", month_abbr(lang, month_index(month)))
}

/// The full name of a month: PT-BR "outubro", EN "October". `month` is 1 to 12.
pub fn month_name(lang: Lang, month: u32) -> String {
    let i = month_index(month);
    match lang {
        Lang::PtBr => MONTH_NAMES_PT_BR[i],
        Lang::En => MONTH_NAMES_EN[i],
    }
    .to_string()
}
