//! Pure time axis for the timeline: where a local time sits on the line, the part of the line a
//! view shows, the background grid, and where "Agora" goes. Nothing here draws or reads the clock.
//!
//! World x is `f64` days since the axis origin, a local calendar day starting at 00:00. Every
//! local day is exactly 1.0 wide, so days stay equal on screen even when daylight saving makes
//! one 23 or 25 hours long. A [`View`] maps world x to screen x as `(x - left) * day_px`. World x
//! is never `f32`: a save years back at the deepest zoom still needs sub-pixel precision.

use chrono::{
    DateTime, Datelike, FixedOffset, Months, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta,
    Timelike,
};
use savestates_core::SaveState;

/// World x: days since the axis origin.
pub type Days = f64;

/// One day on screen at first open.
pub const DEFAULT_DAY_PX: f64 = 140.0;
/// Most zoomed in: one minute is about 40 px.
pub const MAX_DAY_PX: f64 = 57_600.0;
/// Most zoomed out: about two years fit the widget.
pub const MIN_DAY_PX: f64 = 1.2;
/// The grid uses the finest step whose lines are at least this far apart.
pub const GRID_MIN_SPACING_PX: f64 = 60.0;
/// "Agora" stays at least this far right of "Você está aqui".
pub const NOW_MIN_GAP_PX: f64 = 56.0;

const SECONDS_PER_DAY: f64 = 86_400.0;
/// The gap history order leaves after the previous save when a save cannot sit at its own time.
const ONE_SECOND: Days = 1.0 / SECONDS_PER_DAY;

/// Turns a commit time into a local wall-clock time. Production uses the computer's time zone;
/// tests use each time's own offset.
pub type ToLocal = fn(DateTime<FixedOffset>) -> NaiveDateTime;

/// Maps local wall-clock times to world x.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    /// The local day whose 00:00 is world x 0.
    pub origin: NaiveDate,
}

impl Axis {
    /// World x of a local time: whole days since the origin plus the fraction of the day.
    pub fn x_of(&self, local: NaiveDateTime) -> Days {
        let days = (local.date() - self.origin).num_days() as f64;
        days + f64::from(local.time().num_seconds_from_midnight()) / SECONDS_PER_DAY
    }

    /// The local day that contains world x.
    pub fn date_at(&self, x: Days) -> NaiveDate {
        add_days(self.origin, x.floor() as i64)
    }
}

/// `date` moved by `days`, stopping at the ends of the calendar.
fn add_days(date: NaiveDate, days: i64) -> NaiveDate {
    TimeDelta::try_days(days)
        .and_then(|delta| date.checked_add_signed(delta))
        .unwrap_or(if days < 0 {
            NaiveDate::MIN
        } else {
            NaiveDate::MAX
        })
}

/// Where a save sits on the line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: Days,
    /// False when the save's date could not be read; it then just follows the save before it.
    pub dated: bool,
}

/// Places saves (oldest first) at their local commit time; one placement per save, same order.
///
/// History order wins: a save never sits left of the one before it, and one dated earlier sits
/// one second after it. A save with an unreadable date (core's 1970 fallback) sits one second
/// after the previous save, or at now when it is the first. A save dated after now sits at now
/// (then the order rule applies).
pub fn place_saves(
    axis: &Axis,
    saves: &[SaveState],
    to_local: ToLocal,
    now_local: NaiveDateTime,
) -> Vec<Placement> {
    let now_x = axis.x_of(now_local);
    let mut placed: Vec<Placement> = Vec::with_capacity(saves.len());
    for save in saves {
        let after_previous = placed.last().map(|previous| previous.x + ONE_SECOND);
        let dated = save.committed.timestamp() > 0;
        let x = if dated {
            let own = axis.x_of(to_local(save.committed)).min(now_x);
            after_previous.map_or(own, |earliest| own.max(earliest))
        } else {
            after_previous.unwrap_or(now_x)
        };
        placed.push(Placement { x, dated });
    }
    placed
}

/// The part of the line on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Screen pixels per day.
    pub day_px: f64,
    /// World x at the widget's left edge.
    pub left: Days,
    /// The widget's width in pixels.
    pub width: f64,
}

impl View {
    pub fn world_to_screen(&self, x: Days) -> f64 {
        (x - self.left) * self.day_px
    }

    pub fn screen_to_world(&self, sx: f64) -> Days {
        self.left + sx / self.day_px
    }

    /// Pans by a pointer movement of `dx_px`; the content follows the pointer.
    pub fn pan_by(&mut self, dx_px: f64) {
        self.left -= dx_px / self.day_px;
    }

    /// Zooms by `factor` (clamped to the zoom range), keeping the world point under `anchor_px`
    /// where it is.
    pub fn zoom_at(&mut self, factor: f64, anchor_px: f64) {
        let anchor = self.screen_to_world(anchor_px);
        self.day_px = (self.day_px * factor).clamp(MIN_DAY_PX, MAX_DAY_PX);
        self.left = anchor - anchor_px / self.day_px;
    }

    /// A view of the same width with `from..to` centered, filling `fraction` of the width as far
    /// as the zoom range allows. A zero-length span gets the deepest zoom.
    pub fn fit(&self, from: Days, to: Days, fraction: f64) -> View {
        let span = (to - from).abs();
        let day_px = if span > 0.0 {
            (fraction * self.width / span).clamp(MIN_DAY_PX, MAX_DAY_PX)
        } else {
            MAX_DAY_PX
        };
        View {
            day_px,
            left: from.midpoint(to) - self.width / 2.0 / day_px,
            width: self.width,
        }
    }
}

/// How far apart the background grid's lines are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridStep {
    Minutes(u32),
    Hours(u32),
    Days,
    Weeks,
    Months,
}

/// The grid steps from finest to coarsest. Every step under a day divides a day evenly.
const GRID_STEPS: [GridStep; 11] = [
    GridStep::Minutes(1),
    GridStep::Minutes(5),
    GridStep::Minutes(10),
    GridStep::Minutes(15),
    GridStep::Minutes(30),
    GridStep::Hours(1),
    GridStep::Hours(3),
    GridStep::Hours(6),
    GridStep::Days,
    GridStep::Weeks,
    GridStep::Months,
];

impl GridStep {
    /// Days between two lines; a month counts as 30.
    fn spacing_days(self) -> f64 {
        match self {
            GridStep::Minutes(n) => f64::from(n) * 60.0 / SECONDS_PER_DAY,
            GridStep::Hours(n) => f64::from(n) * 3600.0 / SECONDS_PER_DAY,
            GridStep::Days => 1.0,
            GridStep::Weeks => 7.0,
            GridStep::Months => 30.0,
        }
    }
}

/// The finest grid step whose lines are at least [`GRID_MIN_SPACING_PX`] apart; months when
/// even those are closer.
pub fn grid_step(day_px: f64) -> GridStep {
    GRID_STEPS
        .into_iter()
        .find(|step| step.spacing_days() * day_px >= GRID_MIN_SPACING_PX)
        .unwrap_or(GridStep::Months)
}

/// What a grid line marks. `first_of_day` is true on the 00:00 line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GridLabel {
    Minute {
        date: NaiveDate,
        hour: u32,
        minute: u32,
        first_of_day: bool,
    },
    Hour {
        date: NaiveDate,
        hour: u32,
        first_of_day: bool,
    },
    Day(NaiveDate),
    /// The Monday the week starts on.
    Week(NaiveDate),
    Month {
        year: i32,
        month: u32,
    },
}

/// One background grid line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridLine {
    /// Screen x.
    pub x: f64,
    pub label: GridLabel,
}

/// The grid lines at the view's step, in increasing x: from the last line at or before the left
/// edge to the first line at or after the right edge. Lines under a day fall on multiples of the
/// step from 00:00 of each local day; weeks start on Monday and months on day 1.
pub fn grid_lines(axis: &Axis, view: &View) -> Vec<GridLine> {
    let usable = view.day_px > 0.0
        && view.day_px.is_finite()
        && view.left.is_finite()
        && view.width.is_finite();
    if !usable {
        return Vec::new();
    }
    let step = grid_step(view.day_px);
    let right = view.screen_to_world(view.width);
    let mut lines = Vec::new();
    let mut at = first_line(axis, step, view.left);
    loop {
        let x = axis.x_of(at);
        lines.push(GridLine {
            x: view.world_to_screen(x),
            label: label(step, at),
        });
        if x >= right {
            break;
        }
        match next_line(step, at) {
            Some(next) => at = next,
            None => break,
        }
    }
    lines
}

/// The last line of `step` at or before world x `left`.
fn first_line(axis: &Axis, step: GridStep, left: Days) -> NaiveDateTime {
    let day = axis.date_at(left);
    let date = match step {
        GridStep::Minutes(n) => return first_line_in_day(axis, day, n * 60, left),
        GridStep::Hours(n) => return first_line_in_day(axis, day, n * 3600, left),
        GridStep::Days => day,
        GridStep::Weeks => add_days(day, -i64::from(day.weekday().num_days_from_monday())),
        GridStep::Months => day.with_day(1).unwrap_or(day),
    };
    date.and_time(NaiveTime::MIN)
}

/// The last line at or before world x `left` among lines every `step_secs` from 00:00 of `day`,
/// the local day that contains `left`.
fn first_line_in_day(axis: &Axis, day: NaiveDate, step_secs: u32, left: Days) -> NaiveDateTime {
    let midnight = day.and_time(NaiveTime::MIN);
    let into_day = (left - axis.x_of(midnight)) * SECONDS_PER_DAY;
    let slot =
        ((into_day / f64::from(step_secs)).floor().max(0.0) as u32).min(86_400 / step_secs - 1);
    let at = NaiveTime::from_num_seconds_from_midnight_opt(slot * step_secs, 0)
        .map_or(midnight, |time| day.and_time(time));
    // Rounding can put the slot's line a hair past `left`; the one before is then the first.
    if axis.x_of(at) > left {
        at.checked_sub_signed(TimeDelta::seconds(i64::from(step_secs)))
            .unwrap_or(at)
    } else {
        at
    }
}

/// The line of `step` after `at`, or `None` past the end of the calendar.
fn next_line(step: GridStep, at: NaiveDateTime) -> Option<NaiveDateTime> {
    let date = at.date();
    let next = match step {
        GridStep::Minutes(n) => return at.checked_add_signed(TimeDelta::minutes(i64::from(n))),
        GridStep::Hours(n) => return at.checked_add_signed(TimeDelta::hours(i64::from(n))),
        GridStep::Days => date.succ_opt()?,
        GridStep::Weeks => date.checked_add_signed(TimeDelta::weeks(1))?,
        GridStep::Months => date.checked_add_months(Months::new(1))?,
    };
    Some(next.and_time(NaiveTime::MIN))
}

fn label(step: GridStep, at: NaiveDateTime) -> GridLabel {
    let date = at.date();
    let (hour, minute) = (at.hour(), at.minute());
    match step {
        GridStep::Minutes(_) => GridLabel::Minute {
            date,
            hour,
            minute,
            first_of_day: hour == 0 && minute == 0,
        },
        GridStep::Hours(_) => GridLabel::Hour {
            date,
            hour,
            first_of_day: hour == 0,
        },
        GridStep::Days => GridLabel::Day(date),
        GridStep::Weeks => GridLabel::Week(date),
        GridStep::Months => GridLabel::Month {
            year: date.year(),
            month: date.month(),
        },
    }
}

/// Screen x of "Agora": its own time, or [`NOW_MIN_GAP_PX`] right of "Você está aqui"
/// (`head_screen_x`), whichever is further right.
pub fn now_screen_x(view: &View, now_x: Days, head_screen_x: Option<f64>) -> f64 {
    let now = view.world_to_screen(now_x);
    head_screen_x.map_or(now, |head| now.max(head + NOW_MIN_GAP_PX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Weekday};
    use savestates_core::SaveId;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn local(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        date(y, m, d).and_hms_opt(h, mi, s).unwrap()
    }

    /// A commit time at the given local wall-clock time, carrying `offset_h` hours of offset.
    fn committed(offset_h: i32, at: NaiveDateTime) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(offset_h * 3600)
            .unwrap()
            .from_local_datetime(&at)
            .unwrap()
    }

    fn save(n: usize, committed: DateTime<FixedOffset>) -> SaveState {
        SaveState {
            id: SaveId(format!("{n:040x}")),
            name: format!("Save {n}"),
            time: committed,
            committed,
            files_changed: 1,
        }
    }

    /// Saves committed at these times (oldest first), all at UTC-3.
    fn saves_at(times: &[NaiveDateTime]) -> Vec<SaveState> {
        times
            .iter()
            .enumerate()
            .map(|(n, &at)| save(n, committed(-3, at)))
            .collect()
    }

    /// The tests' `ToLocal`: each time's own offset.
    fn own_offset(t: DateTime<FixedOffset>) -> NaiveDateTime {
        t.naive_local()
    }

    #[test]
    fn days_have_equal_width() {
        let axis = Axis {
            origin: date(2026, 10, 20),
        };
        let view = View {
            day_px: DEFAULT_DAY_PX,
            left: 0.0,
            width: 1000.0,
        };
        let a = view.world_to_screen(axis.x_of(local(2026, 10, 26, 0, 0, 0)));
        let b = view.world_to_screen(axis.x_of(local(2026, 10, 27, 0, 0, 0)));
        assert_eq!(b - a, 140.0);
    }

    #[test]
    fn time_within_day() {
        let axis = Axis {
            origin: date(2026, 10, 20),
        };
        let view = View {
            day_px: DEFAULT_DAY_PX,
            left: 0.0,
            width: 1000.0,
        };
        let midnight = view.world_to_screen(axis.x_of(local(2026, 10, 26, 0, 0, 0)));
        let noon = view.world_to_screen(axis.x_of(local(2026, 10, 26, 12, 0, 0)));
        assert_eq!(noon - midnight, 70.0);
    }

    #[test]
    fn midnight_boundary() {
        let axis = Axis {
            origin: date(2026, 10, 20),
        };
        let before = axis.date_at(axis.x_of(local(2026, 10, 31, 23, 59, 0)));
        let after = axis.date_at(axis.x_of(local(2026, 11, 1, 0, 1, 0)));
        assert_eq!(before, date(2026, 10, 31));
        assert_eq!(after, date(2026, 11, 1));
        assert_eq!((after - before).num_days(), 1);
    }

    #[test]
    fn dst_day_keeps_width() {
        let axis = Axis {
            origin: date(2026, 10, 20),
        };
        // Midnight before and after a daylight-saving change: 25 real hours apart.
        let first = committed(-4, local(2026, 11, 1, 0, 0, 0));
        let second = committed(-5, local(2026, 11, 2, 0, 0, 0));
        assert_eq!((second - first).num_hours(), 25);
        let saves = vec![save(0, first), save(1, second)];
        let placed = place_saves(&axis, &saves, own_offset, local(2026, 11, 2, 12, 0, 0));
        assert_eq!(placed[1].x - placed[0].x, 1.0);
    }

    #[test]
    fn history_order_wins() {
        let axis = Axis {
            origin: date(2026, 10, 20),
        };
        let saves = saves_at(&[
            local(2026, 10, 30, 10, 0, 0),
            local(2026, 10, 30, 9, 0, 0),
            local(2026, 10, 30, 11, 0, 0),
        ]);
        let placed = place_saves(&axis, &saves, own_offset, local(2026, 11, 2, 12, 0, 0));
        assert_eq!(placed.len(), 3);
        assert!(placed[0].x < placed[1].x && placed[1].x < placed[2].x);
        assert_eq!(placed[0].x, axis.x_of(local(2026, 10, 30, 10, 0, 0)));
        assert_eq!(placed[1].x, placed[0].x + ONE_SECOND);
        assert_eq!(placed[2].x, axis.x_of(local(2026, 10, 30, 11, 0, 0)));
        assert!(placed.iter().all(|p| p.dated));
    }

    #[test]
    fn unreadable_and_future_dates() {
        let axis = Axis {
            origin: date(2026, 10, 20),
        };
        let now = local(2026, 11, 2, 12, 0, 0);
        let now_x = axis.x_of(now);
        let epoch = DateTime::UNIX_EPOCH.fixed_offset();

        // An unreadable date between two readable ones sits one second after the previous save.
        let saves = vec![
            save(0, committed(-3, local(2026, 10, 30, 10, 0, 0))),
            save(1, epoch),
            save(2, committed(-3, local(2026, 10, 30, 11, 0, 0))),
        ];
        let placed = place_saves(&axis, &saves, own_offset, now);
        assert!(placed[0].dated);
        assert!(!placed[1].dated);
        assert_eq!(placed[1].x, placed[0].x + ONE_SECOND);
        assert!(placed[2].dated);
        assert_eq!(placed[2].x, axis.x_of(local(2026, 10, 30, 11, 0, 0)));

        // As the first save, an unreadable date sits at now.
        let placed = place_saves(&axis, &[save(0, epoch)], own_offset, now);
        assert_eq!(
            placed,
            vec![Placement {
                x: now_x,
                dated: false
            }]
        );

        // A save dated next year is drawn at now.
        let saves = saves_at(&[local(2026, 10, 30, 10, 0, 0), local(2027, 10, 30, 10, 0, 0)]);
        let placed = place_saves(&axis, &saves, own_offset, now);
        assert!(placed[1].dated);
        assert!(placed[1].x <= now_x);
        assert_eq!(placed[1].x, now_x);

        assert!(place_saves(&axis, &[], own_offset, now).is_empty());
    }

    #[test]
    fn precision_far_from_origin() {
        let axis = Axis {
            origin: date(2026, 11, 2),
        };
        let saves = saves_at(&[local(2023, 11, 2, 13, 5, 0), local(2023, 11, 2, 13, 5, 1)]);
        let placed = place_saves(&axis, &saves, own_offset, local(2026, 11, 2, 12, 0, 0));
        let view = View {
            day_px: MAX_DAY_PX,
            left: placed[0].x - 300.0 / MAX_DAY_PX,
            width: 1000.0,
        };
        let first = view.world_to_screen(placed[0].x);
        let second = view.world_to_screen(placed[1].x);
        let gap = second - first;
        assert!(gap > 0.5 && gap < 0.8, "gap {gap}");

        let back = view.screen_to_world(first);
        assert!((view.world_to_screen(back) - first).abs() < 0.01);
        assert!(((back - placed[0].x) * MAX_DAY_PX).abs() < 0.01);
    }

    #[test]
    fn pan_follows_pointer() {
        let mut view = View {
            day_px: 100.0,
            left: 2.0,
            width: 1000.0,
        };
        let under = view.screen_to_world(400.0);
        view.pan_by(50.0);
        assert_eq!(view.left, 1.5);
        assert_eq!(view.screen_to_world(450.0), under);
    }

    #[test]
    fn zoom_keeps_anchor_and_clamps() {
        let mut view = View {
            day_px: DEFAULT_DAY_PX,
            left: -3.0,
            width: 1000.0,
        };
        let anchor = view.screen_to_world(300.0);
        view.zoom_at(2.0, 300.0);
        assert_eq!(view.day_px, 2.0 * DEFAULT_DAY_PX);
        assert!((view.screen_to_world(300.0) - anchor).abs() < 1e-12);

        for _ in 0..100 {
            view.zoom_at(2.0, 300.0);
            assert!((MIN_DAY_PX..=MAX_DAY_PX).contains(&view.day_px));
        }
        assert_eq!(view.day_px, MAX_DAY_PX);
        for _ in 0..100 {
            view.zoom_at(0.5, 300.0);
            assert!((MIN_DAY_PX..=MAX_DAY_PX).contains(&view.day_px));
        }
        assert_eq!(view.day_px, MIN_DAY_PX);
        assert!((view.screen_to_world(300.0) - anchor).abs() < 1e-9);
    }

    #[test]
    fn fit_fills_middle() {
        let view = View {
            day_px: DEFAULT_DAY_PX,
            left: -40.0,
            width: 1000.0,
        };
        let (a, b) = (10.0, 12.0);
        let fitted = view.fit(a, b, 0.6);
        assert_eq!(fitted.width, 1000.0);
        assert!((fitted.world_to_screen(a) - 200.0).abs() < 1e-9);
        assert!((fitted.world_to_screen(b) - 800.0).abs() < 1e-9);

        let point = view.fit(a, a, 0.6);
        assert_eq!(point.day_px, MAX_DAY_PX);
        assert!((point.world_to_screen(a) - 500.0).abs() < 1e-9);
    }

    #[test]
    fn grid_steps() {
        assert_eq!(grid_step(140.0), GridStep::Days);
        assert_eq!(grid_step(30.0), GridStep::Weeks);
        assert_eq!(grid_step(1.2), GridStep::Months);
        assert_eq!(grid_step(57_600.0), GridStep::Minutes(5));
        assert_eq!(grid_step(2_000.0), GridStep::Hours(1));
    }

    #[test]
    fn grid_lines_days() {
        let axis = Axis {
            origin: date(2026, 10, 20),
        };
        let view = View {
            day_px: 140.0,
            left: axis.x_of(local(2026, 10, 26, 0, 0, 0)),
            width: 700.0,
        };
        let expected: Vec<GridLine> = (0..6)
            .map(|i| GridLine {
                x: 140.0 * f64::from(i),
                label: GridLabel::Day(date(2026, 10, 26 + i)),
            })
            .collect();
        assert_eq!(grid_lines(&axis, &view), expected);
    }

    #[test]
    fn grid_lines_weeks_start_monday() {
        let axis = Axis {
            origin: date(2026, 10, 20),
        };
        let view = View {
            day_px: 30.0,
            left: axis.x_of(local(2026, 10, 29, 12, 0, 0)),
            width: 1000.0,
        };
        let lines = grid_lines(&axis, &view);
        assert!(lines.len() >= 6);
        assert!(lines[0].x <= 0.0 && lines[1].x > 0.0);
        assert!(lines[lines.len() - 1].x >= view.width);
        assert!(lines[lines.len() - 2].x < view.width);
        for pair in lines.windows(2) {
            assert!((pair[1].x - pair[0].x - 210.0).abs() < 1e-9);
        }
        for line in &lines {
            let GridLabel::Week(monday) = line.label else {
                panic!("not a week label: {:?}", line.label);
            };
            assert_eq!(monday.weekday(), Weekday::Mon);
            let x = axis.x_of(monday.and_hms_opt(0, 0, 0).unwrap());
            assert_eq!(line.x, view.world_to_screen(x));
        }
    }

    #[test]
    fn grid_lines_hours_mark_midnight() {
        let axis = Axis {
            origin: date(2026, 10, 20),
        };
        let view = View {
            day_px: 2_000.0,
            left: axis.x_of(local(2026, 10, 30, 22, 30, 0)),
            width: 300.0,
        };
        let labels: Vec<GridLabel> = grid_lines(&axis, &view).iter().map(|l| l.label).collect();
        let hour = |d: u32, hour: u32| GridLabel::Hour {
            date: date(2026, 10, d),
            hour,
            first_of_day: hour == 0,
        };
        assert_eq!(
            labels,
            vec![
                hour(30, 22),
                hour(30, 23),
                hour(31, 0),
                hour(31, 1),
                hour(31, 2),
                hour(31, 3)
            ]
        );
    }

    #[test]
    fn grid_lines_minutes_mark_midnight() {
        let axis = Axis {
            origin: date(2026, 10, 20),
        };
        let view = View {
            day_px: MAX_DAY_PX,
            left: axis.x_of(local(2026, 10, 30, 23, 52, 0)),
            width: 600.0,
        };
        let labels: Vec<GridLabel> = grid_lines(&axis, &view).iter().map(|l| l.label).collect();
        let minute = |d: u32, hour: u32, minute: u32| GridLabel::Minute {
            date: date(2026, 10, d),
            hour,
            minute,
            first_of_day: hour == 0 && minute == 0,
        };
        assert_eq!(
            labels,
            vec![
                minute(30, 23, 50),
                minute(30, 23, 55),
                minute(31, 0, 0),
                minute(31, 0, 5),
                minute(31, 0, 10),
            ]
        );
    }

    #[test]
    fn grid_lines_months_start_on_day_one() {
        let axis = Axis {
            origin: date(2026, 10, 20),
        };
        let view = View {
            day_px: MIN_DAY_PX,
            left: axis.x_of(local(2026, 10, 15, 0, 0, 0)),
            width: 200.0,
        };
        let lines = grid_lines(&axis, &view);
        let labels: Vec<GridLabel> = lines.iter().map(|l| l.label).collect();
        let month = |year: i32, month: u32| GridLabel::Month { year, month };
        assert_eq!(
            labels,
            vec![
                month(2026, 10),
                month(2026, 11),
                month(2026, 12),
                month(2027, 1),
                month(2027, 2),
                month(2027, 3),
                month(2027, 4)
            ]
        );
        assert_eq!(
            lines[3].x,
            view.world_to_screen(axis.x_of(local(2027, 1, 1, 0, 0, 0)))
        );
    }

    #[test]
    fn now_gap() {
        let view = View {
            day_px: 100.0,
            left: 0.0,
            width: 1000.0,
        };
        let now_x = view.screen_to_world(505.0);
        assert_eq!(now_screen_x(&view, now_x, Some(500.0)), 556.0);
        assert_eq!(
            now_screen_x(&view, view.screen_to_world(900.0), Some(500.0)),
            900.0
        );
        assert_eq!(
            now_screen_x(&view, now_x, None),
            view.world_to_screen(now_x)
        );
    }
}
