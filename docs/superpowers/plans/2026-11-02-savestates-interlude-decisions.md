# SaveStates interlude: decisions made during implementation

Recorded by the controller while executing `2026-11-02-savestates-interlude.md` (subagent-driven). Each ruling states what was decided, why, and what it costs if wrong. The second section is the backlog of minor review findings deferred to later work.

## Rulings (in the order they were made)

- Ruling: work on branch `interlude` in place (no separate worktree) — same reasoning as wave 1; main untouched — cost if wrong: trivial.
- Ruling I1: implement both `day_label(lang, date, today)` and `day_label_full(lang, date, today, with_month)`; `day_label` delegates with `with_month = false` (first-of-month still shows its month) — plan lists both — cost if wrong: one redundant helper.
- Ruling I2: Task 4 keeps the app compiling with minimal widget/tabs edits: the widget's public method names stay (`set_zoom`/`zoom` temporarily carry the day width as f32↔f64), balls drawn at their new x with no grid/groups; Task 5 renames to `set_day_px`/`day_px`, does the settings rename and all painting — cost if wrong: a little throwaway glue in Task 4.
- Task 1: review → Needs fixes (1 Important: date tests spawn raw unsanitised git commit, duplicated in two crates). Ruling I3: add TestRepo::commit_all_dated (sanitised like the runner) to testkit, used by both tests — cost if wrong: none. Fix round 1 dispatched (resume adbaee7436131cc23). FIX_BASE=db0d2e4.
- Task 2: Ruling I4: brief's weekday examples assumed 2025 dates (2026-10-26 is a Monday); tests use 2025-10-26/27/29 for 'dom 26 out'/'seg 27'/'qua 29 out' — plan arithmetic error, behaviour unchanged — cost if wrong: none.
- Task 3: implementer concern → Ruling I5: leading unreadable-date saves are placed just before the first readable save (1 s steps back), all-unreadable histories end at now; mid-history unreadable keep x[i-1]+1 s — plan's "first unreadable at now" would push every later save past now — cost if wrong: none. Sent before review.
- Task 3: Ruling I6: plan-mandated DST divergence accepted — x_of uses wall-clock seconds/86400 instead of the spec's "÷ length of that day" (ToLocal gives a NaiveDateTime); days stay equal width, the repeated fall-back hour collapses gracefully via the history-order rule — cost if wrong: two saves ~1 h apart on a fall-back night drawn 1 s apart once a year.
- Task 4: Ruling I7: plan arithmetic error accepted — "5 saves 10 px apart → one group" impossible under the anchor rule; test uses 6 px spacing and also pins 10 px → groups 0..3, 3..5 — cost if wrong: none.
- Task 4: Ruling I8: unsplittable-group test adds a later HEAD save (HEAD is pinned, never grouped); layout_range + private now_local/oldest_dated fields accepted — cost if wrong: none.
- Task 4: Ruling I9: spec §4.5 "neighbours never overlap" needs a separation pass after layout (≥2 px between consecutive circles), never moving pinned singles; group hit reach = max(radius, HIT_RADIUS_PX) — sent before review — cost if wrong: small drift of unpinned items from their exact time.
- Task 4: Ruling I10: accepted — per-ball floor (not plain clamp), layout_range without pinned restart points, at-most-two-pinned precondition — cost if wrong: none found.
- Task 5: Ruling I11: accepted capture-driven deviations — pinned balls reserve 15 px in layout (neighbours were hidden under the HERE ring); other-timelines note moved bottom-left (it hid grid labels); pinned saves' labels get room first; group list also closes when the pointer leaves the widget (stands in for clicks elsewhere) — cost if wrong: cosmetic.
- Task 6: Ruling I12: accepted — ui-raven focusable + bold-mono wrappers (Up/Down needs focus), theme.rs cyan #7dd3fc + list background, right-click below rows opens menu without changing selection, thumb drag stops on leave, full path on hover of a truncated name; ListView wrappers left in ui-raven (now unused) — cost if wrong: none.
- Ruling F-1: fix (not accept) the Esc §4.6 deviation — route Esc through the window event hook when the pointer is over the active timeline and no popup is open — spec says Esc must work while the mouse is over the timeline — cost if wrong: small extra hook code.
- Ruling F-2: fix wave also takes cheap extras: cull off-screen balls in paint, accumulate fractional wheel in the group list, tooltip time via state.local, reveal fallback selection only when the selected index changed, rounded row fill, EN hour labels in 24 h ("13:00") to match minute labels and save times, Weak refs in widget timers, drop the dead right-MouseUp branch — cost if wrong: none.
- Ruling F-3: on first open only, if "Você está aqui" would be off-screen at the remembered/default zoom, zoom out so HEAD and Agora both fit (a returning user after a long gap sees their saves) — refines spec §4.3's default — cost if wrong: first view less zoomed-in than the remembered zoom in that case.
- Final: parked — first-view fitted zoom gets saved as the remembered zoom on close (stale project could reopen zoomed out) — Ruling: deferred, cheap follow-up (remember the pre-fit day_px until the user zooms); not load-bearing.
- Final: parked — peek banner converts commit time to now's fixed offset (DST hour mismatch, PT-BR has no DST); focused Raven button keeps Esc when the pointer is elsewhere; a temporary group zoom open at close is remembered; Shared::escape borrow pattern — Ruling: follow-ups, not load-bearing.

## Deferred minor findings (backlog)

- Task 1: minor (deferred): no test for \x1f inside a subject; repeated string-suffix asserts in log test.
- Task 1: minor (deferred): commit_all_dated treats any rev-parse failure as "no parent"; raw date format.
- Task 2: minor (deferred): month_index clamps out-of-range months (test locks it in); day_label doc wording; group keys assume count ≥ 2 (true by construction).
- Task 3: minor (deferred): grid_step untested for Minutes(10/15/30)/Hours(3/6); no negative-x x_of/date_at test; non-finite guard untested; now_gap third assertion tautological.
- Task 4: minor (deferred): widget set_data/append_saved place saves twice and clone all data (Task 5 should fold); layout computed up to 3× per pointer event (cache later).
- Task 4: minor (deferred): no test that replace_data/prepend_older keep an active temporary zoom.
- Task 5: minor (deferred): tooltip time uses now's fixed offset vs labels using state.local (DST hour mismatch); right-edge grid labels clipped; equal-priority label rooms hide a big group's label next to a small one; list scroll rounds fractional wheel deltas; paint.rs 1,656 lines (split GroupList/ListBox, grid helpers); " · "/" – " separators hard-coded in paint.rs; Rtimer_destroy from Drop during another timer callback unverified.
- Task 5: minor (deferred): Esc stays with a focused filter until the timeline is clicked once (by design: no focus steal on hover).
- Task 6: minor (deferred): selected first/last row fill pokes past the rounded outline (visible by default); fallback-to-first-row selection can stay off-screen after a refresh (set reveal in set_rows); scroll-bar/tooltip code mirrors diffpane (shared helper later); no focus indicator; on_select borrow-drop theoretical; peek list right-click selects without a menu.
