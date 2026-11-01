//! Timeline date labels: "hoje 10:42", "ontem", "26 de out.", "26 de out. de 2025".

use chrono::{DateTime, Datelike, FixedOffset, Timelike};

use crate::Lang;

const MONTHS_PT_BR: [&str; 12] = [
    "jan.", "fev.", "mar.", "abr.", "mai.", "jun.", "jul.", "ago.", "set.", "out.", "nov.", "dez.",
];

const MONTHS_EN: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Label for the moment `t`, as seen from `now`.
///
/// `t` is first moved to the offset of `now`, so "today" and "yesterday" are calendar days
/// in `now`'s offset. A `t` in the future (clock skew) follows the same rules: a later day
/// in the same year shows the date.
pub fn relative_date(lang: Lang, t: DateTime<FixedOffset>, now: DateTime<FixedOffset>) -> String {
    let t = t.with_timezone(&now.timezone());
    let day = t.date_naive();
    let today = now.date_naive();

    if day == today {
        let time = format!("{:02}:{:02}", t.hour(), t.minute());
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

    let month = t.month0() as usize;
    let same_year = t.year() == now.year();
    match (lang, same_year) {
        (Lang::PtBr, true) => format!("{} de {}", t.day(), MONTHS_PT_BR[month]),
        (Lang::PtBr, false) => format!("{} de {} de {}", t.day(), MONTHS_PT_BR[month], t.year()),
        (Lang::En, true) => format!("{} {}", MONTHS_EN[month], t.day()),
        (Lang::En, false) => format!("{} {}, {}", MONTHS_EN[month], t.day(), t.year()),
    }
}
