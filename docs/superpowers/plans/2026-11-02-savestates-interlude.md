# SaveStates Interlude Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the timeline into a zoom-on-wheel, time-based day grid with grouped saves and a temporary zoom; replace the change lists' words with colored git symbols.

**Architecture:** The timeline math moves to two pure modules: `timeline/axis.rs` (time to world mapping, the view, grid steps, Agora placement) and `timeline/layout.rs` (grouping and hit testing). `timeline/model.rs` keeps the state machine, now with a temporary-zoom state and the new navigation. The widget only paints the model and animates between views. The change lists keep `ChangesPanel`'s public API, but draw through a small custom-painted row widget whose row logic is pure. Core gains the commit time per save.

**Tech Stack:** Rust 1.96 (edition 2024), Raven v0.2.0 via `savestates-ui-raven`, chrono.

**Spec:** `docs/superpowers/specs/2026-11-02-savestates-interlude-design.md`, an addendum to `docs/superpowers/specs/2026-11-01-savestates-design.md`. Read both. Wave-1 decisions are in `docs/superpowers/plans/2026-11-01-savestates-wave1-decisions.md`.

## Global Constraints

- No user-facing string outside `crates/i18n/translations.csv` (header `key,pt-BR,en`, PT-BR first, plain language, no git words). The one exception is date and time fragments made in `crates/i18n/src/dates.rs` (wave-1 Ruling T9-1). Keys stay literal at call sites so the i18n coverage test sees them.
- The UI thread never runs git or `claude`. `#![forbid(unsafe_code)]` everywhere except `crates/ui-raven`.
- Timeline world coordinates are `f64` **days** relative to an origin day. Never use `f32` for world x: a save years back at max zoom needs sub-pixel precision.
- Spec values (verbatim):
  - Wheel zooms around the cursor with no modifier.
  - Ctrl+wheel also zooms.
  - Shift+wheel and horizontal wheel pan.
  - Drag pans. A click is under 4 px of movement.
  - Every local day has the same width.
  - Max zoom: one minute ≈ 40 px. Min zoom: about two years fit. Default: one day ≈ 140 px.
  - Saves merge when within about two ball widths (28 px) of the group's first member.
  - Agora sits at least 56 px right of "Você está aqui".
  - A clicked group fills the middle 60 %.
  - Pill text "Aproximado · Esc para voltar".
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Format only the crates you touch (`cargo fmt -p <crate>`), never `vendor/`.

## Review Focus

These conditions are the ones most likely to hurt a real user, and each has a test in its owning task:

1. **Many saves in the same minute** (an AI agent plus Quick Save) must stay reachable: the group opens a list at max zoom. *Task 4*.
2. **Out-of-order, corrupt (1970) or future commit dates in imported history** must still draw a line that only moves forward, and must not blow up the range, fit or paging. *Task 3*.
3. **Saves years back, at max zoom, far from the origin day** must still land within 1 px of where they belong (`f64` precision). *Task 3*.
4. **Saves around local midnight and on daylight-saving days** must land in the right day column, and every day must keep the same width. *Task 3*.
5. **Thousands of loaded saves seen at min zoom** must lay out fast enough to pan smoothly: 10,000 saves in under 5 ms. *Task 4*. A change list with thousands of rows must paint only the visible rows. *Task 6*.

---

## File Structure

```
crates/git/src/log.rs                 + committer time fields in LogEntry (%cI, %ct)
crates/core/src/timeline.rs           + SaveState.committed
crates/i18n/src/dates.rs              + grid/date fragment helpers
crates/i18n/translations.csv          + timeline.group.*, timeline.grid.week, timeline.temporary_zoom, change.tip.*
crates/ui-raven/src/event.rs          + Event::Wheel { shift }
crates/app/src/timeline/axis.rs       NEW  time→world mapping, View, grid, Agora placement (pure)
crates/app/src/timeline/layout.rs     NEW  grouping, visible range, hit testing (pure)
crates/app/src/timeline/model.rs      REWRITE  data, state, temporary zoom, navigation, paging (pure)
crates/app/src/timeline/paint.rs      NEW  the painter (moved out of widget.rs, then extended)
crates/app/src/timeline/widget.rs     MODIFY  handle API, animation, Esc, group list, clock
crates/app/src/change_rows.rs         NEW  row model: symbols, path split, middle shortening, scrolling, hit zones (pure)
crates/app/src/change_rows_widget.rs  NEW  custom-painted rows (selection, tooltips, context menu, scroll)
crates/app/src/changes_panel.rs       MODIFY  same pub API, draws through RowsWidget
crates/app/ui/project_tab.xml         MODIFY  the two ListViews become containers for RowsWidget
crates/core/src/settings.rs           timeline_zoom → timeline_day_px
crates/app/src/app.rs, tabs.rs        follow the API changes
```

---

### Task 1: Commit time per save (core)

**Files:**
- Modify: `crates/git/src/log.rs`, `crates/core/src/timeline.rs`
- Test: `crates/git/tests/log.rs`, `crates/core/tests/timeline.rs`

**Interfaces:**
- Produces:
  - `LogEntry` gains `pub committer_time: String` (`%cI`) and `pub committer_unix: Option<i64>` (`%ct`).
  - The log format becomes `%x1e%H%x1f%aI%x1f%at%x1f%cI%x1f%ct%x1f%s`. The subject stays last, so a `\x1f` inside it still lands in the subject.
  - `SaveState` gains `pub committed: chrono::DateTime<chrono::FixedOffset>`, with the same fallback rules as `time` (`%cI`, then `%ct` at UTC, then the Unix epoch at UTC).

- [ ] **Step 1: Write the failing tests**

`crates/git/tests/log.rs`:
- `log_records_committer_time`: commit with `GIT_AUTHOR_DATE=2026-10-01T10:00:00-03:00` and `GIT_COMMITTER_DATE=2026-10-05T12:00:00-03:00`. The entry has `committer_time` ending in `-03:00`, and `committer_unix == Some(<epoch of 2026-10-05T15:00:00Z>)`.

`crates/core/tests/timeline.rs`:
- `save_state_keeps_both_times`: the same commit gives `SaveState.time` = 2026-10-01T10:00-03:00 and `committed` = 2026-10-05T12:00-03:00.
- `malformed_committer_date_falls_back`: forge a commit object with committer zone `+9999` and a valid epoch. `committed` equals that epoch at `+00:00`, and the save still appears.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates-git --test log; cargo test -p savestates-core --test timeline`
Expected: compile errors (fields missing).

- [ ] **Step 3: Implement the fields and the fallback**

Reuse the existing `save_time` logic for both times.

- [ ] **Step 4: Run them to verify they pass**

Run the same commands.
Expected: all pass, including the existing tests.

- [ ] **Step 5: Commit**

`git commit -m "feat(core): read each save's commit time"`

### Task 2: Date fragments and new translation keys (i18n)

**Files:**
- Modify: `crates/i18n/src/dates.rs`, `crates/i18n/src/lib.rs`, `crates/i18n/translations.csv`
- Test: `crates/i18n/tests/dates.rs`

**Interfaces:**
- Produces (in `savestates_i18n`, re-exported from lib):
  - `pub fn day_label(lang: Lang, date: NaiveDate, today: NaiveDate) -> String`: `"hoje"`/`"today"` for today. Otherwise the weekday and day, with the month added on the first of a month and on the first label drawn (the widget passes `with_month`; see the next signature).
  - `pub fn day_label_full(lang: Lang, date: NaiveDate, today: NaiveDate, with_month: bool) -> String`:
    - PT `"dom 26 out"` / `"seg 27"`; EN `"Sun Oct 26"` / `"Mon 27"`.
    - PT weekdays: `dom seg ter qua qui sex sáb`. EN: `Sun Mon Tue Wed Thu Fri Sat`.
  - `pub fn hour_label(lang: Lang, hour: u32) -> String`: PT `"13h"`; EN `"1 PM"`.
  - `pub fn minute_label(hour: u32, minute: u32) -> String`: `"13:05"` (both languages).
  - `pub fn short_date(lang: Lang, date: NaiveDate) -> String`: PT `"31 de out."`; EN `"Oct 31"`. Reuses the existing month tables.
  - `pub fn week_start_label(lang: Lang, monday: NaiveDate) -> String`: PT `"28 set"`; EN `"Sep 28"`.
  - `pub fn month_label(lang: Lang, year: i32, month: u32) -> String`: PT `"out 2026"`; EN `"Oct 2026"`.
  - `pub fn month_name(lang: Lang, month: u32) -> String`: PT `"outubro"`; EN `"October"`.
  - `pub fn time_label(t: NaiveTime) -> String`: `"15:02"`.
- CSV rows (exact):

```
timeline.group.count,{0} saves,{0} save states
timeline.group.day,{0} saves em {1},{0} save states on {1}
timeline.group.week,{0} saves na semana de {1},{0} save states in the week of {1}
timeline.group.month,{0} saves em {1},{0} save states in {1}
timeline.group.range,{0} saves entre {1} e {2},{0} save states between {1} and {2}
timeline.group.hint_zoom,clique para aproximar,click to zoom in
timeline.group.hint_list,clique para ver a lista,click to see the list
timeline.grid.week,semana de {0},week of {0}
timeline.temporary_zoom,Aproximado · Esc para voltar,Zoomed in · Esc to go back
change.tip.new,Novo,New
change.tip.deleted,Apagado,Deleted
change.tip.changed,Alterado,Changed
change.tip.renamed,Renomeado de {0},Renamed from {0}
```

- Weeks start on Monday (ISO). This is the controller's call.

- [ ] **Step 1: Write the failing tests in `crates/i18n/tests/dates.rs`**

Assert every example above in both languages. Plus:
- `day_label_full(PtBr, 2026-11-01, today = 2026-11-01, _) == "hoje"`.
- `with_month = true` on a non-first day gives `"qua 29 out"`.
- 2026-11-01 (a Sunday, `with_month = false`) gives `"dom 1 nov"`, because the first of a month always shows its month.
- `hour_label(En, 0) == "12 AM"` and `hour_label(En, 12) == "12 PM"`.
- `time_label(15:02) == "15:02"`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates-i18n --test dates`
Expected: compile errors.

- [ ] **Step 3: Implement the functions and add the CSV rows**

- [ ] **Step 4: Run all i18n tests**

Run: `cargo test -p savestates-i18n`
Expected: all pass. The csv test checks the new rows, and the coverage test still passes because no app code uses the keys yet.

- [ ] **Step 5: Commit**

`git commit -m "feat(i18n): date fragments for the day grid; group, zoom and change-symbol texts"`

### Task 3: Time axis, view, grid and Agora placement (pure)

**Files:**
- Create: `crates/app/src/timeline/axis.rs`, with unit tests inside
- Modify: `crates/app/src/timeline/mod.rs` (`pub mod axis;`)

**Interfaces:**
- Consumes: `SaveState.committed` (Task 1).
- Produces (`crate::timeline::axis`):
  - `pub type Days = f64;`
  - Zoom constants:
    - `pub const DEFAULT_DAY_PX: f64 = 140.0;`
    - `pub const MAX_DAY_PX: f64 = 57_600.0;` (one minute ≈ 40 px)
    - `pub const MIN_DAY_PX: f64 = 1.2;`
    - `pub const GRID_MIN_SPACING_PX: f64 = 60.0;`
    - `pub const NOW_MIN_GAP_PX: f64 = 56.0;`
  - `pub type ToLocal = fn(DateTime<FixedOffset>) -> NaiveDateTime;`
    - Production passes `|t| t.with_timezone(&chrono::Local).naive_local()`.
    - Tests pass `|t| t.naive_local()`, which uses the time's own offset.
  - The axis: `#[derive(Clone, Copy, Debug, PartialEq)] pub struct Axis { pub origin: NaiveDate }` with:
    - `pub fn x_of(&self, local: NaiveDateTime) -> Days`, defined as `(date − origin).num_days() + seconds_from_midnight / 86_400`.
    - `pub fn date_at(&self, x: Days) -> NaiveDate`.
  - Save placement: `pub struct Placement { pub x: Days, pub dated: bool }` and `pub fn place_saves(axis: &Axis, saves: &[SaveState], to_local: ToLocal, now_local: NaiveDateTime) -> Vec<Placement>`. Saves come oldest first.
    - History order wins: `x[i] = max(true_x[i], x[i−1] + 1 s)`. One second is `1.0 / 86_400.0`.
    - An unreadable date (`committed.timestamp() <= 0`) is placed at `x[i−1] + 1 s` (or at `now` if it is the first save) and has `dated: false`.
    - A future date is clamped to `now`, then the order rule applies.
  - The view: `#[derive(Clone, Copy, Debug, PartialEq)] pub struct View { pub day_px: f64, pub left: Days, pub width: f64 }` with:
    - `world_to_screen(&self, x: Days) -> f64`
    - `screen_to_world(&self, sx: f64) -> Days`
    - `pan_by(&mut self, dx_px: f64)` (content follows the pointer)
    - `zoom_at(&mut self, factor: f64, anchor_px: f64)` (clamped to `MIN_DAY_PX..=MAX_DAY_PX`; the world point under the anchor stays put)
    - `fit(&self, from: Days, to: Days, fraction: f64) -> View` (centers the span; `day_px = fraction * width / (to − from)`, clamped; a zero-length span gives `MAX_DAY_PX`)
  - The grid:
    - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum GridStep { Minutes(u32), Hours(u32), Days, Weeks, Months }`
    - `pub fn grid_step(day_px: f64) -> GridStep`: the finest of 1, 5, 10, 15 and 30 minutes, 1, 3 and 6 hours, day, week and month whose line spacing is at least `GRID_MIN_SPACING_PX`. A month counts as 30 days for the spacing test only.
    - `#[derive(Clone, Copy, Debug, PartialEq)] pub enum GridLabel { Minute { date: NaiveDate, hour: u32, minute: u32, first_of_day: bool }, Hour { date: NaiveDate, hour: u32, first_of_day: bool }, Day(NaiveDate), Week(NaiveDate), Month { year: i32, month: u32 } }`
    - `pub struct GridLine { pub x: f64, pub label: GridLabel }` (x is the screen position)
    - `pub fn grid_lines(axis: &Axis, view: &View) -> Vec<GridLine>`: the lines at the current step that fall inside the view, plus one beyond each edge. Weeks start on Monday, months on day 1.
  - Agora: `pub fn now_screen_x(view: &View, now_x: Days, head_screen_x: Option<f64>) -> f64`, defined as `max(world_to_screen(now_x), head + NOW_MIN_GAP_PX)`.

- [ ] **Step 1: Write the failing unit tests in `axis.rs`**

- `days_have_equal_width`: at `day_px = 140`, 00:00 of two consecutive days are exactly 140 px apart on screen.
- `time_within_day`: 12:00 sits at +70 px from 00:00.
- `midnight_boundary` (Review Focus 4): 23:59 and 00:01 of the next day fall on `date_at` values one day apart.
- `dst_day_keeps_width` (Review Focus 4): local naive times 2026-11-01T00:00 and 2026-11-02T00:00 are exactly 1.0 day apart, whatever the offset the source times carry.
- `history_order_wins` (Review Focus 2): saves with commit times [10:00, 09:00, 11:00] give x strictly increasing. The second is at `x[0] + 1 s`.
- `unreadable_and_future_dates` (Review Focus 2):
  - A 1970-epoch save between two 2026 saves gives `dated == false` and sits at `prev + 1 s`.
  - A save dated next year gives `x <= now_x`.
- `precision_far_from_origin` (Review Focus 3): with the origin today, a save 3 years ago at 13:05:00 vs 13:05:01, at `MAX_DAY_PX`, ends up about 0.67 px apart on screen (assert `> 0.5 && < 0.8`). Its screen x round-trips within 0.01 px.
- `zoom_keeps_anchor_and_clamps`: the world point under x=300 is unchanged after `zoom_at(2.0, 300.0)`. Repeated zooming stays within `[MIN_DAY_PX, MAX_DAY_PX]`.
- `fit_fills_middle`: `fit(a, b, 0.6)` on width 1000 puts `a` at 200 px and `b` at 800 px. A zero-length span gives `MAX_DAY_PX` and is centered.
- `grid_steps`:
  - `grid_step(140.0) == Days`
  - `grid_step(30.0) == Weeks`
  - `grid_step(1.2) == Months`
  - `grid_step(57_600.0) == Minutes(5)` (40 px per minute, so 5 minutes is 200 px ≥ 60, while 1 minute is 40 px < 60)
  - `grid_step(2_000.0) == Hours(1)` (83 px per hour)
- `grid_lines_days`: on width 700 at 140 px per day starting at 00:00 of 2026-10-26, there are lines at 0, 140, … with `Day` labels for 26–31 Oct.
- `grid_lines_weeks_start_monday`: every `Week` label is a Monday.
- `now_gap`:
  - With HEAD at 500 px and now at 505 px, the result is 556.
  - With now at 900 px, the result is 900.
  - With no HEAD, the result is the true now.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates --bin savestates timeline::axis`
Expected: compile errors.

- [ ] **Step 3: Implement `axis.rs` with the signatures above**

Only `axis.rs` changes in this task; it is pure.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p savestates --bin savestates timeline::axis`
Expected: all pass.

- [ ] **Step 5: Commit**

`git commit -m "feat(app): time axis for the timeline: equal days, history-ordered placement, grid steps"`

### Task 4: Grouping, hit testing, temporary zoom and navigation (pure model)

**Files:**
- Create: `crates/app/src/timeline/layout.rs` (with unit tests)
- Rewrite: `crates/app/src/timeline/model.rs` (with unit tests)
- Modify: `crates/ui-raven/src/event.rs` (`Event::Wheel` gains `shift: bool`, read from the modifier bitmask, like `ctrl`) and its smoke and unit tests

**Interfaces:**
- Consumes: Task 3's `axis` items.
- Produces (`crate::timeline::layout`):
  - Constants: `pub const BALL_RADIUS_PX: f64 = 7.0;`, `pub const GROUP_JOIN_PX: f64 = 28.0;`, `pub const GROUP_MAX_RADIUS_PX: f64 = 13.0;`, `pub const HIT_RADIUS_PX: f64 = 12.0;`
  - `#[derive(Clone, Debug, PartialEq)] pub enum Item { Single { index: usize, x: f64 }, Group { members: Range<usize>, x: f64, radius: f64 } }` (x is the screen position)
  - `pub fn group_radius(count: usize) -> f64`: `min(7 + 2·log2(count), 13)`.
  - `pub fn visible_range(xs: &[Days], view: &View) -> Range<usize>`: a binary search, with one screen width of margin on each side.
  - `pub fn layout(view: &View, xs: &[Days], range: Range<usize>, pinned: &[usize]) -> Vec<Item>`:
    1. Walk in index order. A save joins the open group when `screen(x) − screen(first member) < GROUP_JOIN_PX`.
    2. Pinned indices are never members and close the open group.
    3. A group of one becomes `Single`.
    4. A group sits at the middle of its members' screen span.
    5. A group within `radius + BALL_RADIUS_PX + 2` of a pinned ball is shifted away from it.
  - `#[derive(Clone, Debug, PartialEq)] pub enum Hit { Save(usize), Group(Range<usize>), Now }` and `pub fn hit_test(items: &[Item], now_x: f64, x: f64, y: f64, line_y: f64) -> Option<Hit>`: the nearest within the item's radius (`HIT_RADIUS_PX` for singles and Now).
  - `pub fn splits_at(view: &View, xs: &[Days], members: Range<usize>) -> bool`: whether laying out `members` at `view` gives more than one item.
- Produces (`crate::timeline::model`):
  - `Selection` and `TimelineData` stay as they are (oldest-first saves, `more_available`, `unsaved`, `selection`, `other_timelines`).
  - `#[derive(Clone, Debug, PartialEq)] pub enum TimelineAction { Select(Selection), JumpToEnd, RequestOlder, Redraw, Animate { from: View }, OpenGroupList(Range<usize>) }`
  - `#[derive(Clone, Copy, Debug, Default, PartialEq)] pub struct TempZoom { return_to: Option<View> }` with:
    - `begin(&mut self, current: View)`: keeps the existing return point if one is active.
    - `escape(&mut self) -> Option<View>`
    - `accept(&mut self)`
    - `active(&self) -> bool`
  - `pub struct TimelineState { pub data: TimelineData, pub view: View, pub line_y: f64, pub axis: Axis, pub temp: TempZoom, xs: Vec<Days>, now_x: Days, drag: Option<f64>, to_local: ToLocal }` with:
    - `pub fn new(data: TimelineData, width: f64, line_y: f64, to_local: ToLocal, now_local: NaiveDateTime) -> Self`. The axis origin is `now_local.date()`. The view is `DEFAULT_DAY_PX`, with Agora `NOW_MIN_GAP_PX + 24` from the right edge.
    - `pub fn replace_data(&mut self, data: TimelineData)`: re-places the saves and keeps the view.
    - `pub fn prepend_older(&mut self, page_newest_first: Vec<SaveState>, more_available: bool)`: the view does not move, because world x is time-based. The selection index shifts by the number of saves added.
    - `pub fn set_now(&mut self, now_local: NaiveDateTime)`
    - `pub fn set_width(&mut self, width: f64)`: keeps the right edge fixed.
    - `pub fn items(&self) -> Vec<Item>`: the pinned saves are "Você está aqui" and the selected save.
    - `pub fn now_screen_x(&self) -> f64`
    - `pub fn xs(&self) -> &[Days]`
    - `pub fn jump_to_end(&mut self)`: accepts the temporary zoom.
  - `pub fn needs_older_page(s: &TimelineState) -> bool`: true when `more_available` holds and the view's left edge is less than one screen width to the right of the oldest loaded dated save.
  - `pub fn handle_event(s: &mut TimelineState, ev: &Event) -> Vec<TimelineAction>`:
    - Wheel without Shift zooms at the pointer by `1.15^wy`, Ctrl or not. Shift+wheel pans by `wy · 60` px. `wx` pans by `wx · 60` px. Each of these accepts the temporary zoom.
    - Dragging with the left button pans and accepts. A press and release with under 4 px of total movement is a click.
    - A click on `Hit::Save(i)` gives `[Select(Station(i))]` and keeps the temporary zoom.
    - A click on `Hit::Now` gives `[Select(Now)]`.
    - A double click on Now runs `jump_to_end` and gives `[JumpToEnd, Redraw]`.
    - A click on `Hit::Group(m)`:
      - If `splits_at(target, …)` holds, where target is `view.fit(span of m, 0.6)`: call `temp.begin(view)`, set `view = target`, and return `[Animate { from }, Redraw]`.
      - Otherwise return `[OpenGroupList(m)]`.
    - Any view change also adds `RequestOlder` when `needs_older_page` holds.
  - `pub fn on_escape(s: &mut TimelineState) -> Vec<TimelineAction>`: when the temporary zoom is active, `view = return_to` and it returns `[Animate { from }, Redraw]`. Otherwise it returns `[]`.

- [ ] **Step 1: Write the failing unit tests**

In `layout.rs`:
- `separate_saves_stay_single`
- `close_saves_group_from_first_member`: saves 10 px apart, 5 of them, give one group of 5. With 40 saves 20 px apart, every group's member span is under 28 px and there are 20 groups (pairs), never one huge group.
- `group_sits_mid_span_and_radius_caps`: `group_radius(2) == 9`, and `group_radius(1000) == 13`.
- `pinned_never_grouped`: HEAD inside a dense run stays `Single`, the groups on each side don't contain it, and no group's circle overlaps HEAD's circle.
- `hit_prefers_nearest`
- `visible_range_binary_search`
- `layout_is_fast` (Review Focus 5): 10,000 saves 1 minute apart, laid out at `MIN_DAY_PX` over the full range, take under 5 ms (`Instant`, release-independent bound).

In `model.rs`:
- `wheel_zooms_without_modifier`: `Wheel { wy: 1, ctrl: false, shift: false }` multiplies `day_px` by 1.15 around x.
- `shift_wheel_and_wx_pan`
- `drag_pans_and_click_selects`
- `group_click_temporary_zoom_and_escape`:
  1. A click on a group gives `Animate` and an active temp zoom, with the group's first and last members at about 20 % and 80 % of the width.
  2. `on_escape` restores the exact previous view and the temp zoom is inactive.
  3. A second `on_escape` gives `[]`.
- `nested_group_click_keeps_original_return`: two group clicks in a row, then Esc, returns to the view from before the first click.
- `pan_or_zoom_accepts`: after a group click, a wheel event or a drag makes the temp zoom inactive, and Esc then does nothing.
- `peek_keeps_temp_zoom`: clicking a single save during a temp zoom keeps it active.
- `jump_to_end_accepts`
- `unsplittable_group_opens_list` (Review Focus 1): 6 saves with the same commit second give, on click, `[OpenGroupList(0..6)]` at any zoom.
- `now_gap_after_save`: the newest save made 1 minute before `now_local` puts the Agora screen x ≥ HEAD x + 56.
- `older_page_by_time`: with `more_available` set and the view's left edge close to the oldest save, `RequestOlder` is emitted. It is not emitted when the edge is far.
- `prepend_keeps_view_and_shifts_selection`

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates --bin savestates timeline; cargo test -p savestates-ui-raven`
Expected: compile errors.

- [ ] **Step 3: Implement the new model**

Implement `layout.rs`, the new `model.rs` and `Wheel { shift }`. Keep the crate compiling: adapt `timeline/widget.rs` and `tabs.rs` only as far as compiling needs, using placeholder painting of balls at their new x. Task 5 does the real painting. Delete the station-index items that no longer exist (`station_x`, `STATION_SPACING`, `Detail`).

- [ ] **Step 4: Run them to verify they pass, and run the app tests**

Run: `cargo test -p savestates; cargo test -p savestates-ui-raven`
Expected: all pass.

- [ ] **Step 5: Commit**

`git commit -m "feat(app): grouping, temporary zoom and wheel-to-zoom in the timeline model"`

### Task 5: Paint the day grid, groups and temporary zoom (widget)

**Files:**
- Create: `crates/app/src/timeline/paint.rs` (move `Scene` and its helpers out of `widget.rs`, then extend them)
- Modify:
  - `crates/app/src/timeline/widget.rs`
  - `crates/app/src/tabs.rs`
  - `crates/app/src/app.rs`
  - `crates/core/src/settings.rs`
  - `crates/core/tests/settings.rs`
  - `crates/i18n/translations.csv` (only if a new key is needed)

**Interfaces:**
- Consumes: Tasks 2–4.
- Produces:
  - `TimelineWidget` keeps its public methods, except that zoom is now the day width:
    - `set_day_px(&self, px: f64)` and `day_px(&self) -> f64` replace `set_zoom`/`zoom`.
    - New: `escape(&self) -> bool`. It returns true when it ended a temporary zoom; the app calls it on Esc.
    - New: `tick_clock(&self)`. It updates Agora's time, and a 60 s UI timer in the widget calls it.
  - In `Settings`, `timeline_day_px: BTreeMap<String, f32>` replaces `timeline_zoom`. The old key is ignored on load and disappears on the next save (spec §4.9). Update the settings tests.
- Painting (spec §4.2, §4.4–§4.8; colors from `theme.rs`):
  - Grid lines are thin (#1b2440-like `BORDER`), with labels at the top in `MUTED_TEXT`. Today's column gets a soft cyan tint.
  - Labels come from Task 2's helpers. The week label uses `tr_fmt("timeline.grid.week", [week_start_label(..)])`. Labels that would overlap are skipped.
  - Single balls are drawn as in wave 1, at their new x.
  - A group ball is filled `#13284a`-like with a cyan ring and the count centered inside. `tr_fmt("timeline.group.count", [n])` goes under it.
  - The group tooltip is painted on the canvas, like wave 1's station tooltip. Its first line is one of these, picked by the members' span:
    - same day: `tr_fmt("timeline.group.day", [n, short_date])`;
    - same week: `tr_fmt("timeline.group.week", [n, week_start_label])`;
    - same month: `tr_fmt("timeline.group.month", [n, month_name])`;
    - otherwise: `tr_fmt("timeline.group.range", [n, short_date(first), short_date(last)])`.

    Its second line is `tr("timeline.group.hint_zoom")`, or `tr("timeline.group.hint_list")` when the group does not split.
  - Labels under single balls show the name and `time_label`. At week or month grid level the label is `short_date` + time. A label is truncated to fit before the next item, and hidden when there's no room.
  - The pill `tr("timeline.temporary_zoom")` sits bottom-right of the band while `temp.active()`.
  - `Animate { from }` interpolates the view from `from` to the current one over 250 ms with ease-out, using the same timer pattern as the grow-in.
  - `OpenGroupList(m)`: a small popup list under the group, using an in-app dialog or a custom-painted overlay (the builder's choice). It shows `name · time_label` per member, newest first. Picking one gives `Select(Station(i))`. Esc or a click elsewhere closes it.
  - Esc: register a window shortcut through the existing `Window::register_shortcut(Key::Escape, …)` mechanism, routed to the active tab's `TimelineWidget::escape()`. It must not run while a dialog is open; check how `dialogs/` track an open dialog.
- Must survive (spec §4.8):
  - "Você está aqui" ring and label;
  - dashed amber Agora with its count, and the dashed line from HEAD;
  - selection halo; peek on click and `Select(Now)`;
  - paging, triggered by `RequestOlder`, with Task 15's generation rules in `tabs.rs` unchanged;
  - grow-in animation (`append_saved`) and the `history_head` update;
  - the other-timelines note; `set_lang` relabels; the read-only state;
  - "Ir para Agora" and the double-click on Agora.

- [ ] **Step 1: Write the failing tests**

- `crates/core/tests/settings.rs` `day_px_replaces_old_zoom_key`: a file containing `timeline_zoom = { "C:/p" = 1.5 }` loads with an empty `timeline_day_px`. Round-tripping a value works.
- Widget unit tests in `widget.rs`, using the NULL backend pattern Task 14 used (if the pattern doesn't fit the binary, put them in pure helpers in `paint.rs`):
  - `group_tooltip_text_by_span`: the four span cases give the four keys.
  - `escape_ends_temp_zoom_once`.
  - `clock_tick_moves_agora`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates-core --test settings; cargo test -p savestates`
Expected: failures or compile errors.

- [ ] **Step 3: Implement the painting, widget API, settings rename, Esc routing and group list as specified**

- [ ] **Step 4: Run the tests**

Run: `cargo test -p savestates -p savestates-core -p savestates-i18n -p savestates-ui-raven`
Expected: all pass.

- [ ] **Step 5: Visual check with `Window::capture`**

Do this on the sample repo (`pwsh scripts/make-sample-repo.ps1`), plus a repo built with 15 saves inside one afternoon and 6 saves in the same second. Capture these states:
- normal day grid;
- a group with its tooltip;
- the temporary zoom with the pill;
- after Esc;
- the week and month levels;
- max zoom with minutes;
- the group list;
- Agora right after a save, with the gap;
- English.

Put the screenshots in `%TEMP%\tinterlude5\`, view them, and remove the harness before committing.

- [ ] **Step 6: Commit**

`git commit -m "feat(app): day grid timeline with grouped saves and temporary zoom"`

### Task 6: Change list symbols (custom-painted rows)

**Files:**
- Create: `crates/app/src/change_rows.rs` (pure, with unit tests), `crates/app/src/change_rows_widget.rs`
- Modify: `crates/app/src/changes_panel.rs` (same public API), `crates/app/ui/project_tab.xml`, `crates/app/src/main.rs` (modules), `crates/i18n/translations.csv` (remove `change.changed/new/deleted/renamed` if no longer used)

**Interfaces:**
- Consumes: Task 2's `change.tip.*` keys.
- Produces (`crate::change_rows`):
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Symbol { Added, Deleted, Changed, Renamed }`
  - `pub fn symbol(kind: &ChangeKind) -> Symbol`
  - `pub fn symbol_text(s: Symbol) -> &'static str`: `"+"`, `"−"` (U+2212), `"M"`, `"R"`.
  - `pub fn tooltip_key(s: Symbol) -> &'static str`: `"change.tip.new"`, `"change.tip.deleted"`, `"change.tip.changed"`, `"change.tip.renamed"`. For Renamed, the caller formats it with the old path.
  - `pub fn split_path(path: &str) -> (&str, &str)`: the file name and the folder with its trailing `/`. `"a.txt"` gives `("a.txt", "")`.
  - `pub fn shorten_middle(folder: &str, max_px: f64, measure: impl Fn(&str) -> f64) -> String`: keeps the first and last segments, replacing middle segments with `…/` until the result fits. If even `first/…/last/` doesn't fit, it ends in `…`. It never splits inside a character.
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Zone { Symbol(usize), Name(usize), Folder(usize) }`
  - `pub struct RowsModel { pub count: usize, pub row_h: f64, pub scroll_y: f64 }` with:
    - `visible(&self, viewport_h: f64) -> Range<usize>`
    - `row_at(&self, y: f64) -> Option<usize>`
    - `zone_at(&self, x: f64, y: f64, symbol_w: f64, name_end: f64) -> Option<Zone>`
    - `scroll_by(&mut self, dy: f64, viewport_h: f64)`, clamped
    - `ensure_visible(&mut self, row: usize, viewport_h: f64)`
- Produces (`crate::change_rows_widget`): `RowsWidget`, created in code and added with `Widget::add_child` to the XML container that replaces each `ListView`. Behaviour:
  - Each row paints the colored symbol (`ADDED`, `REMOVED`, `UNSAVED` amber, and cyan `#7dd3fc`-like for R), the name in `TEXT`, and the folder right-aligned in `MUTED_TEXT`, shortened with `shorten_middle`.
  - Selection highlight; click selects; Up/Down move the selection when it has focus (my call); wheel scrolls.
  - Hovering the symbol paints its tooltip (`tr(tooltip_key)`, or `tr_fmt("change.tip.renamed", [from])`). Hovering a shortened folder paints the full folder.
  - Right-click selects the row, then opens the context menu (the Agora list only, through the existing menu widget).
  - Read-only does not disable the list; the menu item keeps its existing read-only handling.
- `ChangesPanel` keeps:
  - `new(list_host, on_select)`, `set_changes`, `relabel`, `set_context_menu`, `select_path`, `selected`, `changes`;
  - its selection rules: the selected path is kept, otherwise the first row; a selection made in code is not reported.

  So `tabs.rs` and `peek.rs` don't change beyond the widget name they look up.

- [ ] **Step 1: Write the failing unit tests in `change_rows.rs`**

- `symbols_and_tips`: every `ChangeKind` maps to its symbol, text and key.
- `split_path_cases`: `"css/estilo.css"` gives `("estilo.css", "css/")`, and `"a.txt"` gives `("a.txt", "")`.
- `shorten_middle_keeps_ends`: `"src/app/componentes/botoes/"` with a char-width measure, at a width fitting 20 chars, gives `"src/…/botoes/"`. A wide enough width keeps the folder unchanged. Accented segments are never split.
- `rows_virtualize` (Review Focus 5): with 5,000 rows, `row_h = 24`, viewport 480 and `scroll_y = 24_000`, `visible == 1000..1020` plus at most 1 extra row.
- `zone_hit`: x inside the symbol column gives `Symbol(row)`.
- `scroll_clamps`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates --bin savestates change_rows`
Expected: compile errors.

- [ ] **Step 3: Implement the row model, the widget, the `ChangesPanel` rewiring and the XML change**

- [ ] **Step 4: Run the tests**

Run: `cargo test -p savestates -p savestates-i18n`
Expected: all pass, including the coverage test.

- [ ] **Step 5: Visual check with `Window::capture`**

Capture:
- Agora list and "O que mudou" with all four symbols;
- a symbol tooltip;
- a shortened folder and its hover;
- the right-click menu on a row;
- selection after a refresh.

Put the screenshots in `%TEMP%\tinterlude6\`. Remove the harness before committing.

- [ ] **Step 6: Commit**

`git commit -m "feat(app): change lists show colored git symbols with tooltips"`

### Task 7: (removed)

The owner handles the zip and the guide on their own (spec §6 dropped from this plan).

---

## Spec coverage map

| Spec | Task |
|---|---|
| §3 navigation | 4 (model), 5 (wiring) |
| §4.1 positioning, commit time, history order | 1, 3 |
| §4.2 grid levels and labels | 2, 3, 5 |
| §4.3 zoom range | 3 |
| §4.4 Agora at now, minimum gap | 3, 4, 5 |
| §4.5 grouping, labels, pinned balls | 2, 4, 5 |
| §4.6 temporary zoom, unsplittable list, accept/keep table | 4, 5 |
| §4.7 labels and detail | 5 |
| §4.8 wave-1 behaviors kept | 5 |
| §4.9 persistence key | 5 |
| §5 change symbols | 2, 6 |
| §6 zip + guide | dropped: the owner handles it |
