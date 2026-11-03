# SaveStates wave 2: decisions made during implementation

Recorded by the controller while executing `2026-11-03-savestates-wave2.md` (subagent-driven). Each ruling states what was decided, why, and what it costs if wrong. The second section is the backlog of minor review findings deferred to later work.

## Rulings (in the order they were made)

- Ruling A: in Task 7, the widget's `append_saved`/`prepend_older` stay as glue reimplemented over `replace_data` (rebuild a one-lane TimelineData) until Task 8 removes them — keeps every task compiling — cost if wrong: a little throwaway glue.
- Ruling B: Task 2's `children_sit_under_their_parent_newest_first` gives `a` two saves (m2←a1←a2) and `c` forks from a1, so `a` stays a lane end; expected order stays main, b, a, c — plan's own lane-end rule makes the literal setup a tag — cost if wrong: none.
- Ruling T1-1: log_graph test repo gains a root m0 so the brief's "6 entries" holds — brief arithmetic (5 commits listed) — cost if wrong: none. (Locale concern moot: runner pins LC_ALL=C.)
- Task 2: Ruling T2-1: current branch whose tip is the main line's tip (e.g. `git checkout -b x` on main) is NOT pending — current_lane = main's lane, here = tip, pending_after None (spec §6.5 bullet 3 wins over the brief's "lane end it owns") — cost if wrong: next save's new lane isn't previewed by a pending slot.
- Task 2: Ruling T2-2: also take the reviewer's Minor 1 in the fix: lane ends are decided from children reachable from branch tips only, so a detached HEAD committed on top of branch b doesn't turn b into a tag (keeps lane rule = empty rule) — cost if wrong: none.
- Task 4: Ruling T4-1: accepted beyond-brief safety: on a failed switch the undo resets the index first (git rewrote it before failing); finish_load only replaces/deletes files that still match the origin save exactly — both strictly reduce data-loss risk — cost if wrong: a leftover file edited after Load stays for the user to resolve.
- Task 4: observation: git 2.50 with a locked file exits 0, moves HEAD, leaves the old content → the Ok + left_over path is the real one.
- Task 4: Ruling T4-2 (plan-mandated finding): replace the brief's undo `restore --source=HEAD --worktree -- :/` with restoring only paths in `diff HEAD <target>` whose disk content equals the target blob (or is missing while HEAD has it); others are left and reported — spec anti-goal "never lose work" beats the brief — cost if wrong: a half-written path git did produce stays for the user.
- Task 4: Ruling T4-3: kept_ignored keeps a collapsed folder entry only when `check-ignore` matches the folder itself; otherwise lists its files — avoids permanently hiding new files — cost if wrong: longer exclude lists.
- Task 4: Ruling T4-4: fix round also takes Minors 1–3 (per-file hash retry, case-folded `savestates` clash, re-read HEAD via rev-parse when status fails) — cheap and failure-path related.
- Task 4: NOTE for Task 10: LoadError::SwitchFailed now carries `untouched` paths — the failure dialog should list them.
- Task 7: Ruling T7-1: Rgba gains Debug/PartialEq in ui-raven (one line outside the file list) — needed by model types/tests — cost if wrong: none.
- Task 7: Ruling T7-2: the DetachedHead lane is drawn in MUTED_TEXT (neutral; not a timeline) — Task 8/9 should switch data.rs from lane_color("") — cost if wrong: cosmetic.
- Task 7: NOTE for Task 8: remove #[allow(dead_code)] on data.rs/lane_color/tags once wired.
- Task 8: Ruling T8-1: show_new_save takes only the save id (parent no longer needed; timeline_page removed) — cost if wrong: none.
- Task 8: Ruling T8-2: the sweep runs only when the project is Normal (when_normal) — avoids deleting a branch mid-rebase — cost if wrong: an empty branch lingers until the next normal open.
- Task 8: Ruling T8-3: sample script orders quiet-river's saves after graceful-elephant's (spec §6.2 shared-save rule) — cost if wrong: none.
- Task 8: confirmed gap (enters fix loop): bare repos regress to "history unreadable" because read_timelines runs git status; core must read the current branch without a work tree.
- Task 8: Ruling T8-4: fix round also takes Minor 2 (duplicate older-page requests after relabel/rebuild) and Minor 3 (stale `grow` replaying on an old save) — cheap, user-visible — cost if wrong: none.
- Task 9: Ruling T9-1: branch curve lands at min(fork_x + 72, first_save_x − ball radius) — compressed when the first save is closer (no backwards hook); curves are drawn under labels and labels keep their soft background, so a tight curve never hides text — spec §6.4 "must not cut through labels" read as "never obscures them" — cost if wrong: in very tight cases a curve passes behind a label's box. (Confirmed gap → fix loop.)
- Task 9: Ruling T9-2: accepted Painter::bold_text in ui-raven (no semibold available), lane painting in timeline/paint/lanes.rs, LABEL_TOP 22→16.5, pending name bold instead of italic (no italic) — cost if wrong: cosmetic.
- Task 9: Ruling T9-3: fix round also takes Minors 3–5 (tags of grouped saves in the group tooltip, tag under the name box goes to the tooltip, now_lane by slot index) — cost if wrong: none.
- Task 10: Ruling T10-1: hook warning shown as a second line of the Desfazer toast (a second toast would replace Desfazer); Load flow lives in app/load_glue.rs; one Load at a time per tab; retry dialog lists files; prefer=None on peek Load (lane-owner priority equals load_destination's order when tips coincide) — cost if wrong: cosmetic.
- Task 11: Ruling T11-1: accepted extra key `lane.failed` (generic worker failure text) and the archived dialog closing after an action — cost if wrong: cosmetic.
- Task 11: confirmed gap (enters fix loop): confirming the rename dialog unchanged stores the shown default (e.g. "Linha principal", a translated text) as the description, freezing it across languages — an unchanged answer must be a no-op.
- Final: Ruling F-1: fix I-1 (refused switch puts set-aside ignored files back from <oid>^3, drop ref+marker), I-2 (sweep/empty test: for-each-ref --contains prefilter, drop extra rev-parse, refresh submitted before the sweep), I-3 (left_over = only paths in diff origin..target still matching origin; toast still shown when the dialog closes) — cost if wrong: none.
- Final: Ruling F-2: fix deferred T1 append_info_exclude → append mode (no read_to_string of the whole file).
- Final: Ruling F-3: M-3 — archived branches do NOT count as "other" for the empty test (deviates from spec §3.3's literal "archived or not"; aligns with the lane rule §6.2 and only ever deletes less) — cost if wrong: an empty app branch inside an archived line lingers until unarchived.
- Final: Ruling F-4: recovery entries still marked "set aside by Load" are exempt from the 30-day prune (they are waiting to come back) — cost if wrong: refs accumulate while a friend stays on an old timeline.
- Final: Ruling F-5: also fix M-2 (delete_timeline rolls back ref+config if delete_branch fails), M-6 (LEIA-ME: Apagar/Recuperação and ignored files put away and back), pin a known FNV value in the theme test.
- Final: Ruling F-6: M-1 accepted as is — Arquivar stays in the menu and is refused with the reason in a dialog (Raven menu items aren't disabled per opening here) — cost if wrong: one extra click to learn why.
- Final: parked — M-4 timelines_key omits worktree paths (tooltip staleness only); M-5 every Load lists all ignored files (slow with big node_modules) — Ruling: follow-ups, not load-bearing.
- Final: parked — empty test skips archived branches even when current/main line, while lanes draw those (stray app branch possible after Load+Desfazer on an archived current line) — Ruling: follow-up (align the skip set with lanes.rs's visibility filter); no work lost.
- Final: parked — M-2 rollback deletes the deleted-copy ref when list_branches itself fails (double-failure only) — Ruling: follow-up (keep the copy when unsure).
- Final: parked — after a failed switch where git can't even read the folder, set-aside files stay in Recuperação (marked; come back on a later Load) — Ruling: pre-existing, no loss.
- Final: spec §3.3 amended to match Ruling F-3 (archived branches don't count for "empty").

## Deferred minor findings (backlog)

- Task 1: minor (deferred): config_remove_section matches stderr "no such section" (LC_ALL=C pinned); success path untested.
- Task 1: minor (deferred): on_first_parent_line spawns an extra rev-parse per call; first_parent_count parse failure → 0.
- Task 1: minor (deferred): append_info_exclude rewrites the whole file (not append mode) — crash could truncate info/exclude.
- Task 2: minor (deferred): 50 ms debug perf assertion may flake on a loaded machine.
- Task 2: minor (deferred): a detached-only save can keep shared saves over a branch when it is the older child (read-only state, §4.6).
- Task 3: minor (deferred): read_timelines(limit=0) reports more_available; twin app branches at one tip get no first-sight name.
- Task 4: minor (deferred): a locked file tracked by origin and ignored by target stays on disk unreported in left_over (no loss).
- Task 4: minor (deferred): undo of a refused switch can't delete a target-only file that's in use and doesn't list it in untouched (content is in the target save).
- Task 4: minor (deferred): undo reports type-changed paths (file↔folder) in `untouched` though untouched by anyone; head_now no-saves fallback counts as moved; check-ignore without --literal-pathspecs (":foo" folders).
- Task 5: minor (deferred): restore_deleted creates the branch before config/ref cleanup (a failure + retry makes a -2 duplicate); orphan savestates-deleted.* config sections without a ref are never pruned; list_deleted re-reads refs for the oid; delete_timeline's list_timelines may write a first-sight name.
- Task 7: minor (deferred): scroll-bar drag keeps the temporary zoom (doc says drags end it) — state the rule; ONE_SECOND duplicated in model.rs; fork-in-later-lane ignored silently (add debug_assert); theme test re-derives FNV (pin a known value); short bands may put the line under the header; empty older page path untested (glue).
- Task 8: minor (deferred): reselect duplicates timeline_data's selection; peek "Antes · name" not refreshed on rebuild; double refresh after save; "N linhas" counts the DetachedHead lane.
- Task 8: minor (deferred): a refresh submitted before a save but landing after show_new_save can clear `grow` (animation lost, never replayed).
- Task 9: minor (deferred): timeline scroll-bar thumb has no hover state (change list's does); Agora's two-line "nothing new" text can cross into the next lane.
- Task 9: minor (deferred): with close forks the branch curve is a near-vertical drop rather than the mockup's S (consequence of Ruling T9-1).
- Task 10: minor (deferred): Load during a still-committing Quick Save silently drops the continuation; run_retry not under the one-Load flag (stale Desfazer/left-over dialog possible); show_here survives a failed timelines read; peek Load button clickable while loading; retry dialog lists user-edited files that can never clear (core finish_load reports every status entry).
- Task 11: minor (deferred): lane menu TARGET not cleared on dismiss; when_normal None after a race gives no toast on unarchive/delete/undo.
