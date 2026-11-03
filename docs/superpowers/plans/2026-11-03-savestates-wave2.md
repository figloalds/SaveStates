# SaveStates Wave 2 (Timelines) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every local branch becomes a timeline drawn as a metro-colored lane on the day grid. **Load** moves the project to any save, starting an automatic `savestates/<words>` branch only when it lands mid-line and removing it when it's left empty. Timelines can be renamed, archived, and deleted recoverably.

**Architecture:**
- **git crate.** New branch, config, graph-log and info/exclude plumbing in `crates/git/src/branches.rs`, plus small additions to `log.rs` and `write.rs`. Every call goes through the existing sanitised runner.
- **core: pure parts.** `timelines.rs` holds the domain types. `lanes.rs` decides which save is drawn in which lane, in which order, and where each lane forks. It is pure and works on loaded data.
- **core: operations.** `timelines.rs` also reads the view from git. `load.rs` does Load, `archive.rs` does rename, archive and delete, and `recovery.rs` learns about deleted timelines and Load's set-aside markers.
- **app: model.** The timeline model keeps its flat `saves` vector, now ordered lane by lane, so indexes, groups and `Range<usize>` keep working. Each lane is a range in that vector and has its own y.
- **app: paint and tabs.** Painting gains separators, lane names, curves, tags and the pending lane. The tab reads the timelines view, runs `assign_lanes` on the UI thread (pure and fast), and builds the model data.

**Tech Stack:** Rust 1.96 (edition 2024), system git ≥ 2.31 through `savestates-git`, Raven v0.2.0 through `savestates-ui-raven`, chrono.

**Spec:** `docs/superpowers/specs/2026-11-03-savestates-wave2-design.md`. It is an addendum to `2026-11-01-savestates-design.md` (wave 1) and `2026-11-02-savestates-interlude-design.md`; read all three. The mockups are `docs/superpowers/specs/2026-11-03-savestates-wave2-mockups.html`; open it in a browser. Option B and option C are the approved looks. Earlier rulings are in `docs/superpowers/plans/2026-11-01-savestates-wave1-decisions.md` and `2026-11-02-savestates-interlude-decisions.md`.

Work on the existing branch `wave2`, in place; the spec is already committed there.

## Global Constraints

- **Strings.** No user-facing string may live outside `crates/i18n/translations.csv` (header `key,pt-BR,en`, PT-BR first, plain language, no git words). A branch's own git name may be shown, because it is a name. Keys stay literal at call sites (`tr("…")`, `tr_fmt("…")`, `tr_plural("…")`, `tr_key("…")`) so the coverage test finds them.
- **Threads.** The UI thread never runs git. Pure functions (`assign_lanes`, `timeline_data`) may run on it.
- **Unsafe code.** `#![forbid(unsafe_code)]` everywhere except `crates/ui-raven`.
- **The git runner.** Every git call goes through `Repo::run` and the sanitised environment. Never spawn `git` directly; tests use `TestRepo` helpers.
- **Branch names.** The app-branch prefix is exactly `savestates/` (`APP_PREFIX`). The app never deletes, renames or moves a branch without that prefix on its own. Only explicit user actions do: Apagar, or Load continuing a branch.
- **Exact names on disk:**
  - config keys:
    - `branch.<name>.description`;
    - `branch.<name>.savestates-archived`, value `true`;
    - `savestates.main-line`;
    - `savestates-deleted.<id>.branch` and `savestates-deleted.<id>.name`;
  - refs: `refs/savestates/deleted/<unix>-<n>` and `refs/savestates/load-aside/<unix>-<n>`. The latter mirrors the name of the recovery ref it marks.
  - The comment line in `.git/info/exclude` is exactly `# SaveStates: kept ignored after loading an older save`.
- **Colors** (spec §6.4):
  - The main line is `#38bdf8` (`theme::LINE`).
  - The other timelines use, in this order: `#a78bfa`, `#f472b6`, `#818cf8`, `#e879f9`, `#c4b5fd`, `#93c5fd`. Index = FNV-1a 64-bit hash of the git branch name, mod 6. Do not use `DefaultHasher`: it is not stable across Rust versions.
- **Lane geometry** (mockup): the grid header row is 24 px, a lane is 76 px tall, and the line sits 32 px below the lane's top. The pending lane is a lane-sized slot.
- **Undo toasts:**
  - Load's and archive's Desfazer show for 10 s (`UNDO_SECONDS = 10.0` in `app.rs`).
  - Delete's toast uses the same time.
  - Other notices keep `NOTICE_SECONDS`.
- **Timeline coordinates.** World x stays in `f64` days, as in the interlude.
- **Commits.** Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Formatting.** Run `cargo fmt -p <crate>` only on the crates you touch, never on `vendor/`.

## Review Focus

These are the conditions most likely to hurt a real friend. Each has a test in its owning task.

1. **A file held open by another program during Load (Windows).** Load must end in one of two states:
   - the destination, with the stuck files reported for **Tentar de novo**;
   - the starting save, unchanged.

   It must never leave unreported changes. *Task 4*: `a_locked_file_is_reported_and_finished_later`.
2. **An older save with a shorter `.gitignore`** (`node_modules/`, `.env` left out later). After Load, Agora stays clean and Desfazer is not blocked. *Task 4*: `what_was_ignored_stays_ignored`.
3. **An ignored `.env` that an older save tracks.** It is set aside on the way in and comes back by itself on the way out, with its real content. *Task 4*: `an_ignored_file_in_the_way_is_set_aside_and_comes_back`.
4. **Branches made by other tools:** a timeline merged with `git merge`, a backup branch mid-line, a branch open in another worktree, a branch literally named `savestates`. Each is drawn sensibly, never swept, and Load never fails on it. *Tasks 2, 3, 4*: `a_merged_timeline_keeps_its_lane`, `the_sweep_removes_only_empty_app_timelines`, `a_worktree_branch_tip_starts_a_new_timeline`, `a_branch_named_savestates_blocks_only_new_timelines`.
5. **Big histories.** `assign_lanes` on 10,000 saves and 50 branches must take under 50 ms, and the model's `items()` for 10,000 saves in 8 lanes under 20 ms, in a debug test build. *Tasks 2, 7*.

---

## File Structure

```
crates/git/src/branches.rs          NEW  list/switch/create/delete branches, first-parent line test, own saves
crates/git/src/log.rs               + LogEntry.parents, log_graph (every branch, date order)
crates/git/src/write.rs             + config_set/unset/remove_section/get_regexp, ignored_entries, append_info_exclude
crates/core/src/timelines.rs        NEW  Timeline, GraphSave, TimelinesView; read_timelines, read_older_saves, main line, sweep
crates/core/src/lanes.rs            NEW  assign_lanes (pure): lanes, forks, tags, pending, display order
crates/core/src/names.rs            NEW  savestates/<adjective>-<noun> names
crates/core/src/load.rs             NEW  load, finish_load, continue_from_here, load_destination, kept_ignored
crates/core/src/archive.rs          NEW  rename, archive, list_archived, delete, list_deleted, restore_deleted
crates/core/src/recovery.rs         + load-aside markers, prune of deleted timelines and markers
crates/core/src/model.rs, project.rs  ProjectSnapshot.timelines_key (other_timelines goes in Task 8)
crates/i18n/translations.csv        + timeline.*, load.*, lane.*, archive.*, delete.*, rename.* rows
crates/app/src/theme.rs             + LANE_PALETTE, lane_color
crates/app/src/timeline/data.rs     NEW  TimelineData from TimelinesView + LaneLayout (pure), older-page merge
crates/app/src/timeline/model.rs    lanes: per-lane placement, layout, hit tests, slots, vertical scroll, lane-name hits
crates/app/src/timeline/paint.rs    lanes, names, curves, tags, pending lane, colors, scroll bar, sticky header
crates/app/src/timeline/widget.rs   set_data / set_data_growing, size, lane actions
crates/app/src/tabs.rs              timelines view, refresh key, older pages, selection by id, sweep, Load, lane actions
crates/app/src/peek.rs              Load button, banner names the line
crates/app/src/toast.rs             + show_text
crates/app/src/dialogs/rename.rs    NEW  rename dialog (empty name allowed)
crates/app/src/dialogs/archived.rs  NEW  archived timelines list
crates/app/src/dialogs/recovery.rs  rows for deleted timelines
crates/app/src/app.rs, app/recovery_glue.rs  actions, menus, toasts
crates/app/ui/*.xml                 project_tab (Load buttons), menu_lane.xml NEW, menu_tabs.xml, dialog_rename.xml NEW, dialog_archived.xml NEW
scripts/make-sample-repo.ps1        + timelines for the visual checks
LEIA-ME.md, README.md               a short "Linhas do tempo" section
```

---

### Task 1: Git plumbing for branches, settings, the graph and ignored files

**Files:**
- Create: `crates/git/src/branches.rs`
- Modify: `crates/git/src/log.rs`, `crates/git/src/write.rs`, `crates/git/src/lib.rs`
- Test: `crates/git/tests/branches.rs` (new), `crates/git/tests/log.rs`, `crates/git/tests/write.rs`

**Interfaces:**
- Produces (all re-exported from `savestates_git`):
  - `pub struct BranchInfo { pub name: String, pub tip: String, pub worktree: Option<PathBuf>, pub description: Option<String>, pub archived: bool }`
    - `name` is the short name, e.g. `main` or `savestates/graceful-elephant`.
    - `worktree` is git's `%(worktreepath)`: set for any branch checked out in some worktree, this one included.
    - `description` has its trailing newline trimmed.
    - `archived` is true when `branch.<name>.savestates-archived` is `true`.
  - `pub fn list_branches(repo: &Repo) -> Result<Vec<BranchInfo>, GitError>`
    - Sorted by name. Uses `for-each-ref refs/heads --format=%(refname:strip=2)%00%(objectname)%00%(worktreepath)`, plus one `config -z --get-regexp ^branch\.` call. Split each key on its first `branch.` and its last `.`, because branch names contain dots and slashes.
  - `pub fn switch_to(repo: &Repo, branch: &str) -> Result<(), GitError>`: `switch -q --no-guess <branch>`.
  - `pub fn switch_new(repo: &Repo, branch: &str, at: &str) -> Result<(), GitError>`: `switch -q --no-track -c <branch> <at>`.
  - `pub fn create_branch(repo: &Repo, branch: &str, at: &str) -> Result<(), GitError>`: `branch --no-track <branch> <at>`.
  - `pub fn delete_branch(repo: &Repo, branch: &str) -> Result<(), GitError>`: `branch -D <branch>`. Git also removes the branch's config section.
  - `pub fn on_first_parent_line(repo: &Repo, oid: &str, of: &str) -> Result<bool, GitError>`
    - True when `oid` is on the first-parent line of `of` (a branch or oid).
    - Uses `rev-list --first-parent <of> --not <oid>^@` and looks for `oid` in the output. For a root commit, omit `--not`.
  - `pub fn own_first_parent_saves(repo: &Repo, tip: &str, others: &[String]) -> Result<Vec<String>, GitError>`
    - The oids on `tip`'s first-parent line that are not reachable from any of `others` (tip oids), newest first: `rev-list --first-parent <tip> --not <others…>`.
    - Used for counts and for a timeline's first save. Not used for the "empty" test.
  - `pub fn first_parent_count(repo: &Repo, rev: &str) -> Result<u64, GitError>`: `rev-list --count --first-parent`.
  - `LogEntry` gains `pub parents: Vec<String>` (`%P`, split on spaces; empty for a root). The format becomes `%x1e%H%x1f%P%x1f%aI%x1f%at%x1f%cI%x1f%ct%x1f%s`, with the subject still last. `log_first_parent` uses the same format.
  - `pub fn log_graph(repo: &Repo, skip: usize, limit: usize) -> Result<Vec<LogEntry>, GitError>`
    - Command: `log --branches [HEAD] --date-order -z --name-only --diff-merges=first-parent --format=<fmt> --skip=<skip> -n <limit> --`.
    - `HEAD` is passed only when it resolves to a commit (`resolves_to_commit`), so a detached save is listed and an unborn branch is no error.
    - Newest first; `files` counts against the first parent, merges included.
  - `pub fn config_set(repo: &Repo, key: &str, value: &str) -> Result<(), GitError>`: `config --local <key> <value>`.
  - `pub fn config_unset(repo: &Repo, key: &str) -> Result<(), GitError>`: `config --local --unset <key>`. Exit code 5 (not set) is `Ok`.
  - `pub fn config_remove_section(repo: &Repo, section: &str) -> Result<(), GitError>`: `config --local --remove-section`. A missing section is `Ok`.
  - `pub fn config_get_regexp(repo: &Repo, pattern: &str) -> Result<Vec<(String, String)>, GitError>`: `config -z --get-regexp`. Each record is `key\nvalue\0`. Exit code 1 (none) gives an empty `Vec`.
  - `pub fn ignored_entries(repo: &Repo) -> Result<Vec<String>, GitError>`: `ls-files -z --others --ignored --exclude-standard --directory`. Ignored folders come out collapsed, ending in `/`.
  - `pub fn append_info_exclude(repo: &Repo, comment: &str, patterns: &[String]) -> Result<(), GitError>`
    - The file is `git_path(repo, "info/exclude")`; create its folder if it is missing.
    - Append a newline when the file doesn't end in one, then `comment`, then one pattern per line.
    - Does nothing for empty `patterns`.

- [ ] **Step 1: Write the failing tests**

`crates/git/tests/branches.rs` (build repos with `TestRepo`; make branches with `t.git(&["branch", …])`):
- `list_branches_reads_tips_descriptions_and_flags`. Setup: `main` with 2 saves; `savestates/a` at the first save with description `Versão com menu lateral`; `develop` with `savestates-archived = true`. Expect:
  - 3 entries in name order;
  - each `tip` equals `rev-parse <name>`;
  - `savestates/a`'s description is exactly `Versão com menu lateral`;
  - only `develop` is archived;
  - `worktree` is `Some` only for `main`.
- `list_branches_marks_other_worktrees`: `git worktree add <tmp>/wt -b side`. `side.worktree` names that folder (compare with `dunce::canonicalize`).
- `switch_new_and_switch_to_move_the_project`:
  - `switch_new(repo, "savestates/x", <first save>)` puts `HEAD` on `refs/heads/savestates/x` at that oid, and the files match it.
  - `switch_to(repo, "main")` goes back.
- `delete_branch_removes_its_settings`: after `delete_branch`, `config_get(repo, "branch.savestates/a.description")` is `None`.
- `first_parent_line_ignores_merged_in_saves`. Setup: main `m1`; `feature` `f1`, `f2` from `m1`; main `m2`; `merge --no-ff feature` → `m3`. Expect:
  - `on_first_parent_line(f2, "main") == false`;
  - `on_first_parent_line(m2, "main") == true`;
  - `on_first_parent_line(m3, "main") == true`;
  - `on_first_parent_line(m1, "feature") == true`.
- `own_first_parent_saves_stop_at_other_tips`: in that repo, `own_first_parent_saves(feature_tip, [main_tip])` is empty (merged in), and `own_first_parent_saves(main_tip, [feature_tip])` is `[m3, m2]`.

`crates/git/tests/log.rs`:
- `log_graph_lists_every_branch_with_parents`, on the merge repo above:
  - 6 entries, newest first;
  - every child is listed before its parents;
  - the merge has 2 `parents`;
  - the merge's `files` counts the merged file (first-parent diff).
- `log_graph_pages_with_skip_and_limit`: `skip=2, limit=3` gives entries 2..5 of the full list.
- `log_graph_of_an_empty_repo_is_empty`: `TestRepo::new()` with no commit gives `Ok(vec![])`.
- `log_graph_includes_a_detached_save`: `checkout --detach main`, commit `d1`; `d1` is listed.

`crates/git/tests/write.rs`:
- `config_set_unset_and_regexp`:
  - setting then reading `branch.savestates/a.description` round-trips accents;
  - `config_unset` twice is `Ok`;
  - `config_get_regexp("^branch\\.")` returns the keys set;
  - `config_remove_section("savestates-deleted.1-0")` on a missing section is `Ok`.
- `ignored_entries_collapse_folders`: `.gitignore` = `node_modules/\n*.log\n`, plus files `node_modules/p/i.js`, `node_modules/q.js`, `x.log` and `src/a.txt` (tracked). The result is exactly `["node_modules/", "x.log"]`, sorted.
- `append_info_exclude_adds_a_commented_block`: the exclude file ends with `\n# c\n/node_modules/\n/.env\n`, and `git check-ignore -q .env` succeeds.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates-git`
Expected: compile errors (functions and field missing).

- [ ] **Step 3: Implement `branches.rs` and the `log.rs`/`write.rs` additions with the signatures above; re-export them from `lib.rs`**

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p savestates-git && cargo test -p savestates-core`
Expected: all pass, the existing log tests included (the new format must not break `log_first_parent`).

- [ ] **Step 5: Commit**

`git commit -m "feat(git): branches, local settings, the graph log and ignored-file listing"`

### Task 2: Timeline types and lane assignment (pure core)

**Files:**
- Create: `crates/core/src/timelines.rs` (types and pure helpers only in this task), `crates/core/src/lanes.rs`
- Modify: `crates/core/src/lib.rs`
- Test: unit tests in `crates/core/src/lanes.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces (re-exported from `savestates_core`):
  - `pub const APP_PREFIX: &str = "savestates/";`
  - `pub struct Timeline { pub branch: String, pub tip: SaveId, pub display_name: Option<String>, pub archived: bool, pub main_line: bool, pub current: bool, pub elsewhere: Option<PathBuf> }`, with `pub fn app_made(&self) -> bool`. `elsewhere` is the folder of another worktree the branch is checked out in.
  - `pub struct GraphSave { pub save: SaveState, pub parents: Vec<SaveId> }`
  - `pub struct TimelinesView { pub timelines: Vec<Timeline>, pub head: Option<SaveId>, pub detached: bool, pub saves: Vec<GraphSave>, pub more_available: bool }`. `timelines` is every local branch, by name. `saves` is newest first, as `log_graph` lists them.
  - `pub enum LaneOwner { Branch(String), DetachedHead }`
  - `pub struct Lane { pub owner: LaneOwner, pub saves: Vec<usize>, pub fork: Option<usize>, pub continues_past_loaded: bool }`. `saves` and `fork` are indexes into `TimelinesView::saves`; `saves` runs oldest first.
  - `pub struct Tag { pub branch: String, pub save: usize }`
  - `pub struct LaneLayout { pub lanes: Vec<Lane>, pub tags: Vec<Tag>, pub here: Option<usize>, pub current_lane: Option<usize>, pub pending_after: Option<usize> }`
    - `lanes` is in display order (spec §6.3).
    - `here` is HEAD's save, as an index into `saves`.
    - `pending_after` is the lane the pending lane sits right after.
  - `pub fn assign_lanes(view: &TimelinesView) -> LaneLayout`

- [ ] **Step 1: Write the failing tests**

Build views with a small helper, e.g. `fn view(saves: &[(&str, &[&str], &str)], branches: &[(&str, &str)], main: &str, current: Option<&str>) -> TimelinesView`. Each save is `(oid, parents, "YYYY-MM-DD HH:MM")` and each branch is `(name, tip)`; flags are set by small mutators. Then assert on oids, not indexes, through a helper `lane_oids(&layout, &view, lane) -> Vec<&str>`.
- `a_single_line_is_one_lane`: m1←m2←m3 on `main` gives one lane `[m1,m2,m3]`, `fork None`, `here = m3`, `current_lane = Some(0)`.
- `a_timeline_from_the_middle_forks_off_it`: main m1..m4; `savestates/x` = m2←x1←x2.
  - Lanes: `[m1..m4]` and `[x1,x2]`.
  - Lane 1's fork is m2.
- `children_sit_under_their_parent_newest_first`:
  - Setup: main m1..m5; `a` from m2 (a1 at 10:00); `b` from m3 (b1 at 12:00); `c` from a1 (c1 at 11:00).
  - Display order: main, b, a, c. That's depth-first under main, with b first because it is newer than a, and c right after a.
- `shared_saves_stay_with_the_line_that_continued_first`:
  - Setup: main m1..m3; `a` = m1←s1←a1 (a1 committed 09:00); `c` = s1←c1 (c1 10:00).
  - `a`'s lane is `[s1,a1]`; `c`'s lane is `[c1]`, forking at s1.
  - Swap the times and `c` keeps s1 instead.
- `a_branch_inside_another_line_is_a_tag`:
  - `develop` at d1 and `feature` = d1←f1 (both off main): `feature`'s lane is `[d1,f1]`; `develop` is `Tag { develop, d1 }`.
  - `backup` at m2 mid-main is a tag on m2.
- `two_branches_at_one_tip_share_a_lane`: `savestates/x` and `zeta` both at x1. Owner `savestates/x` (app-made before other names); `zeta` is a tag.
- `a_merged_timeline_keeps_its_lane`: the Task 1 merge shape. Main's lane is `[m1,m2,m3]`; `feature`'s lane is `[f1,f2]` with fork m1.
- `merged_saves_without_a_branch_are_not_drawn`: same, with `feature` deleted. Only main's lane; f1 and f2 appear in no lane and no tag.
- `the_main_line_stays_on_top_even_when_branched_at_its_tip`: `feature` = m3←f1 where m3 is main's tip.
  - Lane 0 is main `[m1,m2,m3]`.
  - Lane 1 is `[f1]`, forking at m3.
- `a_just_loaded_timeline_is_pending`: current `savestates/x` at m2 (mid-main).
  - `pending_after = Some(0)`, `here = m2`, `current_lane = None`.
  - No tag for `savestates/x`.
- `archived_timelines_are_hidden_unless_current`:
  - `a` (archived) = m1←a1←a2 and `c` = a1←c1: no lane for `a`; `c`'s lane is `[a1,c1]`, forking at m1.
  - Make `a` current: `a` gets a lane again.
- `a_detached_save_gets_its_own_lane`: detached HEAD at d1, which is on no branch (d1's parent m2). A lane with `owner DetachedHead` holds `[d1]`, forking at m2, and `here = d1`.
- `a_line_leaving_the_loaded_saves_continues`: `x`'s first parent is not loaded. `continues_past_loaded = true`, `fork None`, and the lane sits after main's subtree.
- `nothing_saved_yet_gives_no_lanes`: empty `saves` gives an empty layout.
- `lanes_for_ten_thousand_saves_are_quick`: 10,000 saves on main plus 49 branches of 20 saves each, forking at spread points. `assign_lanes` takes under 50 ms (`Instant`); every save is in exactly one lane.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates-core --lib lanes`
Expected: compile errors.

- [ ] **Step 3: Implement the types and `assign_lanes`**

The rules (spec §6.2, §6.3, §6.5, corrected §6.2). These are the decisions the tests pin:

```
visible tips  = timelines not archived, plus the current one even if archived,
                plus HEAD as LaneOwner::DetachedHead when view.detached
chain saves   = loaded saves reachable from a visible tip through first parents (within loaded)
children[p]   = chain saves whose first parent is p
main chain    = the main line's first-parent line within loaded saves
heir(p)       = p's main-chain child, when p is on the main chain and has one;
                else its oldest child by `committed`, ties by smaller oid
lane ends     = the main line's tip (always), and every other visible tip with no children
owner of an end (when several tips share it) = main line, then current, then app-made, then name;
                the rest at that save become tags
lane saves    = walk from the end through first parents; take a parent p only while heir(p)
                is the save just taken; else fork = Some(p) and stop. A first parent that is
                not loaded sets continues_past_loaded
tags          = visible tips that are not lane ends (their save lies inside a lane), except the
                current branch when it is pending
pending       = the current branch exists and its tip is loaded but is not a lane end it owns:
                pending_after = the lane holding that save, here = that save, current_lane = None
otherwise     = current_lane = the lane the current branch owns; here = its tip
                (detached: the DetachedHead lane, or the lane holding HEAD's save)
display order = main first; then depth-first: under each lane, the lanes forking off it, newest
                first by the `committed` time of their oldest own save; lanes with no fork
                (continues_past_loaded, or no main line) after main's subtree, newest first
```

Build index maps once: oid → index, and children lists. The walk must stay linear in the number of saves.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p savestates-core --lib lanes`
Expected: all pass.

- [ ] **Step 5: Commit**

`git commit -m "feat(core): timelines and lane assignment: which save is drawn in which lane"`

### Task 3: Reading timelines, the main line, names at first sight, the refresh key and the sweep

**Files:**
- Modify: `crates/core/src/timelines.rs`, `crates/core/src/model.rs`, `crates/core/src/project.rs`, `crates/core/src/timeline.rs` (share `into_save_state`), `crates/core/src/lib.rs`
- Test: `crates/core/tests/timelines.rs` (new), `crates/core/tests/project.rs`

**Interfaces:**
- Consumes: Task 1 (`list_branches`, `log_graph`, `config_*`, `on_first_parent_line`, `own_first_parent_saves`, `first_parent_count`, `delete_branch`) and Task 2 types.
- Produces:
  - `pub const MAIN_LINE_KEY: &str = "savestates.main-line";`
  - `pub fn read_timelines(p: &Project, limit: usize) -> Result<TimelinesView, CoreError>`
    - Reads the branches and the newest `limit` saves (`more_available = len == limit`).
    - Flags `current` from `status().branch`, and set `elsewhere = worktree` for every branch that isn't the current one (a branch is checked out in one place at most).
    - Resolves the main line (below).
    - Writes the display name of every app-made branch that has no description and at least one own save (`own_first_parent_saves(tip, other tips)` is non-empty). The name is the subject of the oldest of those saves. It is written with `config_set(branch.<name>.description)` and returned in `display_name`.
  - `pub fn read_older_saves(p: &Project, skip: usize, limit: usize) -> Result<Vec<GraphSave>, CoreError>`
  - `pub fn list_timelines(p: &Project) -> Result<Vec<Timeline>, CoreError>`
    - The branch part of `read_timelines` (main line and first-sight names included), without history. `read_timelines` is `list_timelines` plus `log_graph`.
  - `pub fn is_empty_app_timeline(p: &Project, branch: &str) -> Result<bool, CoreError>`
    - True for an app-made branch whose tip `on_first_parent_line` of some other local branch. Archived branches count as other branches.
    - False for non-app branches.
  - `pub fn sweep_empty_timelines(p: &Project) -> Result<Vec<String>, CoreError>`
    - Deletes every empty app branch that is not current and has no `worktree`.
    - Returns their names.
  - `ProjectSnapshot` gains `pub timelines_key: String`: HEAD oid, current branch, and the main-line value, then one line per branch with `name tip archived description`. Equal keys mean the timelines look the same.
  - `other_timelines` stays until Task 9, now computed as "more than one branch" from the same listing.
  - Main-line rule (spec §3.2), `fn main_line(p, branches, current) -> Result<Option<String>, CoreError>`:
    - If `savestates.main-line` names an existing branch, use it.
    - If the key is unset, pick, in order: the current branch when it is not app-made; else `main`; else `master`; else the branch with the largest `first_parent_count` (ties by name). Write the pick to the key.
    - If the key is set but its branch is gone, apply the same fallback without the current-branch step, and do not rewrite the key.

- [ ] **Step 1: Write the failing tests**

`crates/core/tests/timelines.rs`:
- `the_main_line_is_remembered_at_first_read`: on `main` with `savestates/x`, reading writes `savestates.main-line = main`, and only `main` has `main_line`.
- `opened_on_an_app_branch_the_main_line_is_main`: current `savestates/x`, with `main` present, gives `main`.
- `without_main_or_master_the_longest_line_is_the_main_line`: current `savestates/x`, branches `a` (3 saves) and `b` (5) give `b`.
- `a_missing_main_line_falls_back_without_rewriting`: key = `gone` gives `main`, and the key still reads `gone`.
- `an_app_timeline_is_named_after_its_first_own_save`:
  - `savestates/x` with saves `Versão com menu lateral` then `Ajustes` gets that first name as its description.
  - `develop` (foreign) gets none.
  - An existing description is never overwritten.
- `history_comes_in_pages`: 7 saves with `limit 5` give 5 saves and `more_available`; `read_older_saves(5, 5)` gives the 2 oldest.
- `the_sweep_removes_only_empty_app_timelines`. Setup:
  - `savestates/empty` at a mid-main save (not checked out);
  - `savestates/kept` with an own save;
  - `savestates/merged`, joined into main with `git merge --no-ff`;
  - `backup` (foreign) mid-main;
  - `savestates/wt`, empty but checked out in another worktree;
  - the current branch `savestates/here`, empty.

  Only `savestates/empty` is removed.
- `a_crash_after_load_is_tidied_by_the_next_sweep`: `switch -c savestates/x <mid save>`, then `switch main`. The sweep returns `["savestates/x"]`.

`crates/core/tests/project.rs`:
- `the_timelines_key_follows_refs_names_and_flags`. Each of these changes the key:
  - a new branch;
  - a moved branch;
  - a description;
  - the archived flag;
  - a switch.

  Editing a file doesn't.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates-core --test timelines --test project`
Expected: compile errors.

- [ ] **Step 3: Implement the functions and the key**

`read_snapshot` calls `list_branches` once and builds both `timelines_key` and `other_timelines` from it.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p savestates-core && cargo build -p savestates`
Expected: all pass, and the app still builds.

- [ ] **Step 5: Commit**

`git commit -m "feat(core): read timelines across branches, remember the main line, sweep empty ones"`

### Task 4: Load

**Files:**
- Create: `crates/core/src/load.rs`, `crates/core/src/names.rs`
- Modify: `crates/core/src/recovery.rs` (markers; the helpers Load needs become `pub(crate)`), `crates/core/src/lib.rs`
- Test: `crates/core/tests/load.rs` (new); unit tests in `load.rs` and `names.rs`

**Interfaces:**
- Consumes: Tasks 1–3. Also `recovery::{set_aside_ignored_colliding, restore_recovery, list_recovery}` and `ls_tree_names`.
- Produces:
  - `pub fn new_app_branch_name(taken: &[String], seed: u64) -> String`
    - Format: `savestates/<adjective>-<noun>`, from built-in lists of at least 40 lowercase ASCII words each (friendly words: `graceful`, `elephant`).
    - Tries 8 seeded picks; then adds `-2`, `-3`… to the last pick until it is free.
    - The caller passes a seed from the clock's nanoseconds.
  - `pub enum Destination { Switch(String), Start }`
  - `pub fn load_destination(timelines: &[Timeline], target: &SaveId, prefer: Option<&str>) -> Destination`
    - Candidates are timelines that are not archived, not current, `elsewhere.is_none()`, and whose `tip == target`.
    - Order: `prefer`, then main line, then app-made, then by name.
    - No candidate gives `Start`.
  - `pub const KEEP_IGNORED_COMMENT: &str = "# SaveStates: kept ignored after loading an older save";`
  - `pub fn exclude_pattern(entry: &str) -> String`: an anchored, literal gitignore pattern.
    - Prefix `/`.
    - Backslash-escape `\ * ? [` anywhere, and `#` or `!` at the start.
    - Escape trailing spaces as `\ `.
    - Keep a trailing `/`.
  - `pub fn kept_ignored(before: &[String], after_untracked: &[String]) -> Vec<String>`
    - Returns the `before` entries that cover some `after` path. An entry covers a path when it is equal to it, or when the entry is a folder (`…/`) that holds it.
    - Sorted and deduplicated.
  - `pub struct LoadOrigin { pub save: SaveId, pub branch: Option<String> }`
  - `pub struct LoadOutcome`, with these fields:
    - `pub origin: LoadOrigin`;
    - `pub branch: String`, the branch now current;
    - `pub started: bool`;
    - `pub set_aside: Option<RecoveryEntry>`;
    - `pub brought_back: usize`;
    - `pub kept_ignored: Vec<String>`;
    - `pub left_over: Vec<String>`;
    - `pub removed: Option<String>`;
    - `pub warning: Option<String>`.
  - `#[derive(thiserror::Error, Debug)] pub enum LoadError { HasUnsavedChanges, NotNormal, PrefixTaken, SwitchFailed { details: String }, Core(#[from] CoreError) }`
  - `pub fn load(p: &Project, target: &SaveId, prefer: Option<&str>) -> Result<LoadOutcome, LoadError>`
  - `pub fn finish_load(p: &Project, origin: &SaveId) -> Result<Vec<String>, CoreError>`: **Tentar de novo**. Returns the paths still left over.
  - `pub fn continue_from_here(p: &Project) -> Result<String, LoadError>`
    - Spec §4.6: only when the only blocker is `Detached`.
    - Uses `load_destination` on HEAD's save. `Switch(b)` → `switch_to(b)`; `Start` → `switch_new(new name, HEAD)`.
    - Unsaved changes are allowed and carried along.
    - Returns the branch name.
  - `pub const LOAD_ASIDE_PREFIX: &str = "refs/savestates/load-aside/";` in `recovery.rs`, with:
    - `pub(crate) fn mark_load_aside(p, entry: &RecoveryEntry)`;
    - `pub(crate) fn bring_back_load_aside(p) -> Result<usize, CoreError>`.
- What `load` does, in this order (spec §4.3):
  1. Check that `snapshot(p).condition == Normal`, else `NotNormal`. Check that `status` has no entries, else `HasUnsavedChanges`.
  2. Run `list_timelines(p)` and compute `load_destination`. When the result is `Start` and a branch named exactly `savestates` exists, return `PrefixTaken`.
  3. Record `origin` (HEAD, current branch).
  4. Set aside ignored files in the way: `set_aside_ignored_colliding(p, &ls_tree_names(target))`. Mark the entry with `mark_load_aside`, which creates `refs/savestates/load-aside/<same suffix>` pointing at the same oid.
  5. `before = ignored_entries(repo)`.
  6. Switch: `switch_to` or `switch_new(new_app_branch_name(..), target)`. When git returns an error, read HEAD again.
     - **HEAD did not move:** put the folder back to the source save. That is `restore --source=HEAD --worktree -- :/`, plus deleting untracked files that the target tracks and the source doesn't. Then return `SwitchFailed { details: stderr }`.
     - **HEAD moved:** carry on, with `warning = Some(stderr)`. A failing `post-checkout` hook lands here.
  7. Take the status's untracked paths, compute `kept_ignored(before, them)`, and pass the result through `exclude_pattern` into `append_info_exclude(KEEP_IGNORED_COMMENT, …)`.
  8. `left_over` = the paths of `status` entries still present. The folder was clean before, so each one is a file git couldn't update.
  9. When `left_over` is empty, run `bring_back_load_aside`:
     - For each marker whose recovery ref still exists and whose files are all free (not in `ls_tree_names(HEAD)` and absent on disk): `restore_recovery`, then delete the marker.
     - Delete markers whose recovery ref is gone.
  10. When `origin.branch` is app-made, differs from the new branch, and `is_empty_app_timeline` holds, delete it (`removed`).
- `finish_load(p, origin)` handles each status entry:
  - **Tracked:** `restore_worktree(HEAD, ":(top,literal)<path>")`.
  - **Untracked and in `ls_tree_names(origin)`:** delete the file. Its content is in the origin save.

  Then run `bring_back_load_aside` when the folder is clean, and return what is left.

- [ ] **Step 1: Write the failing tests**

Unit tests (`load.rs`, `names.rs`):
- `destination_continues_a_timeline_ending_at_the_save`, `destination_starts_mid_line`, `destination_order_is_prefer_main_app_name`, `destination_skips_archived_current_and_elsewhere`.
- `exclude_patterns_are_anchored_and_literal`:
  - `node_modules/` → `/node_modules/`;
  - `.env` → `/.env`;
  - `[id].js` → `/\[id].js`;
  - `a*b?.txt` → `/a\*b\?.txt`;
  - `#notes` → `/\#notes`;
  - `!x` → `/\!x`;
  - `end ` → `/end\ `.
- `kept_ignored_maps_new_files_back_to_what_was_ignored`:
  - before: `["node_modules/", ".env", "logs/a.log"]`;
  - after: `["node_modules/x/y.js", "node_modules/z.js", ".env", "src/new.txt"]`;
  - result: exactly `[".env", "node_modules/"]`. `src/new.txt` was never ignored, and `logs/a.log` didn't show up. Patterns are made separately, by `exclude_pattern`.
- `new_names_are_prefixed_words_avoiding_taken_ones`:
  - 200 names from different seeds all match `^savestates/[a-z]+-[a-z]+$` and avoid `taken`.
  - With every combination of 2×2 test lists taken (through a `pub(crate) fn name_from(adjs, nouns, taken, seed)`), the result ends in `-2`.

`crates/core/tests/load.rs` (`TestRepo`; main = m1..m3 unless stated):
- `loading_mid_line_starts_a_new_timeline`: `load(m2)` puts HEAD at m2 on a branch matching `^savestates/`, with `started`, `origin == (m3, Some("main"))`, and files that match m2.
- `loading_the_last_save_of_a_timeline_switches_to_it`: `exp` at e2 (from m1); `load(e2)` gives current `exp` and `!started`.
- `unsaved_changes_block_load_and_touch_nothing`:
  - A modified tracked file gives `Err(HasUnsavedChanges)`, with HEAD and file content unchanged.
  - A new, not-ignored file gives the same.
- `leaving_an_empty_app_timeline_deletes_it`: `load(m2)`, then `load(m3)` gives current `main` and `removed == Some(x)`; `x` is gone.
- `a_timeline_with_a_save_is_kept_when_left`: `load(m2)`, Quick Save (`commit_all`), `load(m3)`. `removed == None`, and `x` exists with a description.
- `desfazer_returns_to_where_the_friend_was`: `o = load(m2)?.origin`; `load(o.save, o.branch.as_deref())` puts us back on `main` at m3.
- `an_ignored_file_in_the_way_is_set_aside_and_comes_back`:
  - Setup: m1 tracks `.env` = `old`; m2 removes it from the saves and adds `.env` to `.gitignore`; on disk `.env` = `real`.
  - `load(m1)`: `.env` reads `old`, `set_aside.files == [".env"]`, and a marker exists.
  - `load` main's tip: `.env` reads `real`, `brought_back == 1`, the entry is gone from `list_recovery`, and the marker is gone.
- `a_set_aside_file_waits_in_recovery_while_its_path_is_taken`:
  - Setup as above, plus branch `old2` from m1 with a save that still tracks `.env`.
  - `load(m1)`, then `load(old2 tip)`: `brought_back == 0`, and the entry is still listed.
- `what_was_ignored_stays_ignored`:
  - Setup: m1 has no `.gitignore` line for `node_modules/`; m2 adds `node_modules/`; on disk `node_modules/p/i.js` and `node_modules/q.js`.
  - `load(m1)`: `status` has no entries; `kept_ignored == ["/node_modules/"]`; `.git/info/exclude` contains `KEEP_IGNORED_COMMENT` then `/node_modules/`.
  - `load(origin)` succeeds (not blocked).
- `a_locked_file_is_reported_and_finished_later` (`#[cfg(windows)]`):
  - Setup: m3 changes `a.txt`. Hold it open with `std::os::windows::fs::OpenOptionsExt::share_mode(0)` so git can neither rewrite nor delete it. `load(m1)`.
  - **First**, run this once by hand and note what git 2.50 does. Then pin it. The test asserts the review-focus rule:
    - either `Ok` with `left_over == ["a.txt"]` and HEAD at m1;
    - or `Err(SwitchFailed)` with HEAD at m3 and an empty `status`.
  - In the `Ok` case: drop the handle; `finish_load(&m3)` returns `[]`, `status` is empty, and `a.txt` matches m1.
- `a_failing_post_checkout_hook_is_only_a_warning`: a `post-checkout` hook that exits 1 (a `#!/bin/sh` script; git for Windows runs it). Load is `Ok`, with `warning.is_some()` and HEAD at the target.
- `a_branch_named_savestates_blocks_only_new_timelines`:
  - With a branch named `savestates`, `load(m2)` is `Err(PrefixTaken)`, and nothing changed.
  - `load(exp tip)` still switches.
- `a_worktree_branch_tip_starts_a_new_timeline`: `side`, checked out in another worktree, at s1. `load(s1)` gives `started` on a new `savestates/…` branch.
- `continuing_from_a_bare_save`:
  - Detached at m2 with a modified file: `continue_from_here` creates `savestates/…` at m2, and the modification is still there.
  - Detached at m3 (main's tip): it switches to `main`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates-core --test load; cargo test -p savestates-core --lib load names`
Expected: compile errors.

- [ ] **Step 3: Implement `names.rs`, `load.rs` and the markers as specified above**

`prune_recovery` changes in Task 5.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p savestates-core`
Expected: all pass, the earlier recovery and bring-back tests included.

- [ ] **Step 5: Commit**

`git commit -m "feat(core): Load: travel to any save, automatic timelines, ignored files kept safe"`

### Task 5: Rename, archive, delete and restore timelines

**Files:**
- Create: `crates/core/src/archive.rs`
- Modify: `crates/core/src/recovery.rs` (`prune_recovery` also prunes deleted timelines and load-aside markers), `crates/core/src/lib.rs`
- Test: `crates/core/tests/archive.rs` (new), `crates/core/tests/recovery.rs`

**Interfaces:**
- Produces:
  - `#[derive(thiserror::Error, Debug)] pub enum ArchiveError { MainLine, Current, Elsewhere(PathBuf), NotFound, Core(#[from] CoreError) }`
  - `pub fn rename_timeline(p: &Project, branch: &str, name: &str) -> Result<(), ArchiveError>`
    - Trims `name`. An empty name runs `config_unset(description)` to restore the default; anything else sets the description.
  - `pub fn archive_timeline(p: &Project, branch: &str) -> Result<(), ArchiveError>`
    - Refuses the main line, the current branch, and a branch with a `worktree`.
  - `pub fn unarchive_timeline(p: &Project, branch: &str) -> Result<(), ArchiveError>`
  - `pub struct ArchivedTimeline { pub branch: String, pub display_name: Option<String>, pub own_saves: usize, pub last_save: SaveState }`
  - `pub fn list_archived(p: &Project) -> Result<Vec<ArchivedTimeline>, CoreError>`
    - Newest `last_save.committed` first.
    - `own_saves` = `own_first_parent_saves(tip, other tips).len()`.
  - `pub const DELETED_PREFIX: &str = "refs/savestates/deleted/";`
  - `pub struct DeletedTimeline { pub ref_name: String, pub branch: String, pub display_name: Option<String>, pub deleted_unix: i64 }`
  - `pub fn delete_timeline(p: &Project, branch: &str) -> Result<DeletedTimeline, ArchiveError>`
    - Refuses like archiving.
    - Writes `refs/savestates/deleted/<unix>-<n>` → tip, plus `savestates-deleted.<unix>-<n>.branch`/`.name`, then `delete_branch`.
  - `pub fn list_deleted(p: &Project) -> Result<Vec<DeletedTimeline>, CoreError>`: newest first.
  - `pub fn restore_deleted(p: &Project, ref_name: &str) -> Result<String, CoreError>`
    - `create_branch` under the old name, or the first free `<name>-2`, `<name>-3`…
    - Puts back the description, sets archived, then removes the ref and its config section.
    - Returns the branch name used.
  - `prune_recovery(p, now_unix)` also removes deleted timelines (ref and section) and load-aside markers older than 30 days, or whose recovery ref is gone. It still returns the total removed.

- [ ] **Step 1: Write the failing tests**

`crates/core/tests/archive.rs`:
- `renaming_sets_and_clears_the_display_name`: `develop` → `Versão do Claude` (accents survive); then `""` unsets it. The branch is still named `develop`.
- `archive_hides_and_unarchive_shows`: `read_timelines` reports `archived` accordingly.
- `archive_refuses_main_current_and_elsewhere`: `MainLine`, `Current`, `Elsewhere(_)`.
- `archived_timelines_list_own_saves_and_last_save`: `savestates/x` with 3 own saves gives `own_saves == 3`, and `last_save` is its tip.
- `delete_keeps_a_recoverable_copy`:
  - Delete archived `savestates/x` (display `Versão com menu`): the branch is gone, and `list_deleted` has one entry with both names.
  - `restore_deleted` puts the branch back at the same tip, with its description and `archived == true`; `list_deleted` is empty and the config section is gone.
- `restoring_picks_a_free_name`: with `savestates/x` re-created meanwhile, the restore is named `savestates/x-2`.
- `delete_refuses_main_and_elsewhere`.

`crates/core/tests/recovery.rs`:
- `prune_removes_old_deleted_timelines_and_markers`:
  - Run `prune_recovery(now + 31 days)`: it removes a deleted timeline (ref and section) and an old load-aside marker.
  - Run with `now + 1 day`: both are kept.
  - An orphan marker is removed either way.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates-core --test archive --test recovery`
Expected: compile errors.

- [ ] **Step 3: Implement `archive.rs` and the prune additions**

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p savestates-core`
Expected: all pass.

- [ ] **Step 5: Commit**

`git commit -m "feat(core): rename, archive and recoverably delete timelines"`

### Task 6: Translation rows for wave 2

**Files:**
- Modify: `crates/i18n/translations.csv`
- Test: `crates/i18n/tests/coverage.rs` (unchanged; it must keep passing as later tasks use the keys)

**Interfaces:**
- Produces these CSV rows, exactly (PT-BR first). Also replace the `condition.detached` row with the second block's version.

```
timeline.main_line,Linha principal,Main line
timeline.pending,Nova linha do tempo · começa no seu próximo save,New timeline · starts with your next save
timeline.no_line,Sem linha do tempo,No timeline
timeline.archived_tag,arquivada,archived
timeline.lines.one,{n} linha,{n} timeline
timeline.lines.other,{n} linhas,{n} timelines
peek.banner_on_line,Espiando {0} · {1} · em {2},Peeking at {0} · {1} · on {2}
load.button,Carregar este save,Load this save
load.tip_continue,Continua a linha do tempo {0},Continues the timeline {0}
load.tip_start,Seus próximos saves vão começar uma nova linha do tempo a partir daqui,Your next saves will start a new timeline from here
load.tip_elsewhere,"Esta linha do tempo está aberta em outra pasta, então seus próximos saves vão começar uma nova linha do tempo a partir daqui","This timeline is open in another folder, so your next saves will start a new timeline from here"
load.unsaved_title,Alterações não salvas,Unsaved changes
load.unsaved_body,Você tem alterações não salvas. Salve antes de carregar outro save.,You have unsaved changes. Save before loading another save.
load.save_first,Salvar primeiro,Save first
load.done,Você carregou {0},You loaded {0}
load.done_aside,"Você carregou {0}. Arquivos ignorados que estavam no caminho foram guardados e voltam quando você sair deste save.","You loaded {0}. Ignored files that were in the way were put away and come back when you leave this save."
load.left_over_title,Alguns arquivos não foram atualizados,Some files weren't updated
load.left_over_body,Alguns arquivos não puderam ser atualizados porque outro programa está usando eles. Feche esse programa e clique em Tentar de novo.,Some files couldn't be updated because another program is using them. Close that program and click Try again.
load.retry,Tentar de novo,Try again
load.failed_title,Não foi possível carregar,Couldn't load
load.failed,Não foi possível carregar este save. Nada foi perdido.,This save couldn't be loaded. Nothing was lost.
load.prefix_taken,"Este projeto tem uma linha do tempo chamada ""savestates"", então não dá para começar uma nova linha do tempo aqui.","This project has a timeline named ""savestates"", so a new timeline can't be started here."
load.hook_warning,"Save carregado, mas as verificações automáticas deste projeto reclamaram de algo.","Save loaded, but this project's automatic checks complained about something."
lane.rename,Renomear,Rename
lane.archive,Arquivar linha do tempo,Archive timeline
lane.cant_archive_title,Não dá para arquivar,Can't archive
lane.archive_main,A linha principal não pode ser arquivada.,The main line can't be archived.
lane.archive_current,Carregue outro save antes de arquivar esta linha do tempo.,Load another save before archiving this timeline.
lane.elsewhere,Esta linha do tempo está aberta em outra pasta ({0}).,This timeline is open in another folder ({0}).
lane.archived,Linha do tempo arquivada.,Timeline archived.
rename.title,Renomear linha do tempo,Rename timeline
rename.hint,Deixe vazio para voltar ao nome original.,Leave it empty to go back to the original name.
archive.open,Linhas do tempo arquivadas…,Archived timelines…
archive.title,Linhas do tempo arquivadas,Archived timelines
archive.empty,Nenhuma linha do tempo arquivada.,No archived timelines.
archive.restore,Restaurar,Restore
archive.delete,Apagar…,Delete…
delete.title,Apagar linha do tempo,Delete timeline
delete.body,Apagar a linha do tempo '{0}' ({1})?,Delete the timeline '{0}' ({1})?
delete.confirm,Apagar,Delete
delete.done,Linha do tempo apagada. Ela fica na Recuperação por 30 dias.,Timeline deleted. It stays in Recovery for 30 days.
recovery.deleted_timeline,Linha do tempo apagada: {0},Deleted timeline: {0}
recovery.timeline_restored,Linha do tempo restaurada. Ela está nas linhas do tempo arquivadas.,Timeline restored. It's in the archived timelines.
```

```
condition.detached,"Este projeto está olhando um save antigo, aberto por outro programa. Clique em Carregar este save para continuar daqui, ou volte para a linha do tempo por lá e clique em Verificar de novo.","This project is looking at an old save from another program. Click Load this save to continue from here, or go back to the timeline there and click Check again."
```

- [ ] **Step 1: Add the rows; check that the CSV parses and the PT-BR column is complete**

- [ ] **Step 2: Run the i18n tests**

Run: `cargo test -p savestates-i18n`
Expected: pass.

- [ ] **Step 3: Commit**

`git commit -m "feat(i18n): texts for timelines, Load, archive and delete"`

### Task 7: Lanes in the timeline model (pure app)

**Files:**
- Create: `crates/app/src/timeline/data.rs`
- Modify:
  - `crates/app/src/timeline/model.rs`
  - `crates/app/src/timeline/layout.rs` (only if a lane-scoped `layout_range` needs a helper)
  - `crates/app/src/timeline/mod.rs`
  - `crates/app/src/theme.rs`
  - Minimal glue so the app still builds: `crates/app/src/timeline/paint.rs` and `crates/app/src/timeline/widget.rs` draw every ball at its lane's y with straight lines; `crates/app/src/tabs.rs` builds one lane from `timeline_page` as before.
- Test: unit tests in `data.rs`, `model.rs`, `theme.rs`

**Interfaces:**
- Consumes: Task 2's `TimelinesView`, `LaneLayout`, `LaneOwner`.
- Produces:
  - `theme::LANE_PALETTE: [Rgba; 6]` (the Global Constraints values) and `pub fn lane_color(branch: &str, main_line: bool) -> Rgba`.
  - In `model.rs`:
    - `pub struct LaneData { pub label: String, pub branch: Option<String>, pub color: Rgba, pub saves: Range<usize>, pub fork: Option<usize>, pub continues_left: bool, pub archived: bool }`
      - `saves` indexes `TimelineData::saves`, and so does `fork` (the save it branches off).
      - `archived` is set on the current timeline when it is archived.
    - `TimelineData` becomes `{ saves: Vec<SaveState>, lanes: Vec<LaneData>, tags: Vec<(usize, String)>, here: Option<usize>, current_lane: Option<usize>, pending_after: Option<usize>, more_available: bool, unsaved: usize, selection: Selection }`.
      - `saves` is lane by lane in display order, each lane oldest first.
      - `other_timelines` is removed; `you_are_here()` returns `here`.
    - `pub fn TimelineData::one_lane(saves_oldest_first: Vec<SaveState>, more_available: bool, unsaved: usize) -> TimelineData`. This is glue and test helper: one lane labelled with `""`, here = newest.
  - In `data.rs`:
    - `pub struct LaneNames { pub main_line: String, pub no_line: String }`
    - `pub fn timeline_data(view: &TimelinesView, layout: &LaneLayout, names: &LaneNames, unsaved: usize, selected: Option<&SaveId>) -> TimelineData`
      - Each label is `display_name`, else `names.main_line` for the main line, else the git branch name; `names.no_line` is used for `DetachedHead`.
      - Colors come from `lane_color`.
      - The selection is the index of `selected` when present, else `Now`.
    - `pub fn merge_older(view: &mut TimelinesView, older: Vec<GraphSave>, more_available: bool)`: appends in order, skipping ids already loaded.
  - `TimelineState` geometry and API changes:
    - `pub const HEADER_PX: f64 = 24.0`, `LANE_PX: f64 = 76.0` and `LINE_OFFSET_PX: f64 = 32.0`. The `line_y` field goes.
    - New public methods:
      - `set_size(&mut self, width: f64, height: f64)` replaces `set_width`; it keeps the right edge as before and clamps the scroll;
      - `pub fn lane_of(&self, i: usize) -> Option<usize>`;
      - `pub fn lane_line_y(&self, lane: usize) -> f64`;
      - `pub fn now_line_y(&self) -> f64`, the pending slot when there is one, else the current lane, else slot 0;
      - `pub fn slot_count(&self) -> usize`, the lanes plus a pending slot;
      - `pub fn scroll_y(&self) -> f64`;
      - `pub fn set_label_widths(&mut self, widths: Vec<f64>)`, which the widget measures with `text_width`.
    - The slots are the lanes in order, with the pending slot inserted right after `pending_after`. A slot's line y is `HEADER_PX + slot × LANE_PX + LINE_OFFSET_PX − scroll_y`.
    - Placement:
      - Per lane, `axis::place_saves` on the lane's saves.
      - A lane with a `fork` starts no further left than the fork save's x. Lanes come after their parents in display order, so place them in order.
      - `needs_older_page` judges the oldest dated save across all lanes.
    - `items()` lays out each lane on its own (`layout_range` and `layout` on the lane's index range); groups never span lanes. The pinned saves are `here` and the selected save.
    - `hit_at(x, y)`:
      - Pick the slot from `y`.
      - In a lane slot, hit-test only that lane's items, with Agora when it sits in that slot.
      - A point within a lane's name box (x in `0..label_width + 24`, y in the lane's top 18 px) is `Hit::LaneName(lane)`.
    - New `Hit::LaneName(usize)`. New actions:
      - `TimelineAction::LaneMenu { lane: usize, x: f32, y: f32 }` on a right-button release over a lane name;
      - `TimelineAction::Rename(usize)` on a left double-click on a lane name.
    - Scrolling and navigation:
      - A drag pans both ways (dy scrolls lanes, clamped to `0..=max(0, content − height)`).
      - When the content is taller than the height, a press within the right 10 px drags a scroll bar: the thumb moves the scroll proportionally.
      - The plain wheel still zooms.
    - `jump_to_end` and `first_view` also scroll so Agora's slot is in view.
    - `replace_data` keeps the view and the scroll (clamped). `append_save` and `prepend_older` are removed: new data always comes through `replace_data`.

- [ ] **Step 1: Write the failing tests**

`theme.rs`:
- `lane_colors_are_stable_and_the_main_line_is_cyan`:
  - `lane_color("main", true) == LINE`;
  - `lane_color("savestates/graceful-elephant", false)` equals the palette entry at FNV-1a64 mod 6 (compute the expected index in the test);
  - the same name always gives the same color.

`data.rs`:
- `saves_are_lane_by_lane_oldest_first`: two lanes, `[m1,m2,m3]` and `[x1,x2]`, give saves `m1 m2 m3 x1 x2`, lane ranges `0..3` and `3..5`, and `fork == Some(index of m2)`.
- `labels_use_display_main_and_git_names`: a description wins; the main line without one shows `names.main_line`; `develop` shows `develop`.
- `the_selection_follows_its_save`: `selected = x1` gives `Station(3)`; a missing id gives `Now`.
- `older_saves_merge_without_duplicates`.

`model.rs`:
- `lanes_stack_under_the_header`: 3 lanes at scroll 0 give line ys 56, 132, 208.
- `the_pending_slot_sits_after_its_parent`: `pending_after = Some(0)` with 3 lanes gives `now_line_y() == 132` and `lane_line_y(1) == 208`.
- `a_branch_starts_after_its_fork`: a child's first save dated before its fork save is placed just right of the fork x.
- `hits_stay_in_their_own_lane`: saves at the same time in lanes 0 and 1; a click at lane 1's y selects lane 1's save.
- `groups_never_cross_lanes`: two close saves, one per lane, stay two items.
- `right_click_on_a_lane_name_opens_its_menu` and `double_click_on_a_lane_name_renames`.
- `vertical_drag_scrolls_within_bounds`: 6 lanes in a 200 px tall view; dragging up by 1,000 px clamps to the max, and dragging down clamps to 0.
- `jump_to_end_reveals_agoras_slot`.
- `here_need_not_be_the_newest_save`: `here` mid-lane with a pending slot; Agora's x respects the 56 px gap from `here`.
- `older_pages_are_judged_across_lanes`.
- `ten_thousand_saves_in_eight_lanes_lay_out_quickly`: `items()` takes under 20 ms.
- Keep every existing model test passing, adapted to `TimelineData::one_lane`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates timeline::`
Expected: compile errors.

- [ ] **Step 3: Implement the lane model, `data.rs`, `lane_color` and the glue**

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p savestates`
Expected: all pass, and the app builds and runs showing one lane as before.

- [ ] **Step 5: Commit**

`git commit -m "feat(app): lanes in the timeline model: per-lane placement, grouping, hits and scrolling"`

### Task 8: The tab shows real timelines

**Files:**
- Modify:
  - `crates/app/src/tabs.rs`
  - `crates/app/src/peek.rs`
  - `crates/app/src/timeline/widget.rs` (the API below)
  - `crates/app/src/timeline/paint.rs` (only to remove the "other timelines" note)
  - `crates/core/src/model.rs` and `crates/core/src/project.rs` (remove `other_timelines`)
  - `crates/core/src/timeline.rs` (remove `timeline_page`, keep `HISTORY_PAGE`)
  - `crates/git/src/log.rs` (remove `log_first_parent` if nothing uses it)
  - `crates/i18n/translations.csv` (drop `timeline.other_timelines_note`)
  - `crates/app/src/app.rs`, `crates/app/src/app/quick_save_glue.rs` (the new save's grow-in)
  - `scripts/make-sample-repo.ps1`
- Test: unit tests in `tabs.rs` (`TabState`), `crates/core/tests/project.rs` (field removal)

**Interfaces:**
- Consumes: Tasks 3 and 7.
- Produces:
  - `TimelineWidget`:
    - `set_data(&self, d: TimelineData)` keeps the view.
    - `set_data_growing(&self, d: TimelineData, new_save: usize)` runs the grow-in for that save. It replaces `append_saved`.
    - `set_unsaved(&self, unsaved: usize)` loses its `other_timelines` parameter.
    - `pub fn lane(&self, lane: usize) -> Option<LaneData>`.
    - `pub fn show_here_and_now(&self)` runs the model's `first_view`.
    - `prepend_older` is removed.
    - The widget passes its height to `set_size`, and measures lane labels for `set_label_widths` after `set_data` and `set_lang`.
    - The owner hears `LaneMenu` and `Rename` as well as the existing actions. The tab ignores both until Task 11.
  - `TabState` keeps:
    - `timelines: Option<TimelinesView>`, the loaded view;
    - `shown_key: Option<String>`, which replaces `history_head`;
    - `selected: Option<SaveId>`, the selection by id across reloads;
    - `grow: Option<SaveId>`, the save just made, which the next data grows in.
  - `fn needs_history(shown_key: &Option<String>, key: &str) -> bool`
  - Refresh:
    - The worker runs `snapshot`. When the key differs, it also runs `read_timelines(p, max(HISTORY_PAGE, loaded))`.
    - On the UI: `assign_lanes`, then `timeline_data` with `LaneNames { tr("timeline.main_line"), tr("timeline.no_line") }`, then `set_data`, or `set_data_growing` when `grow` is in the new data.
    - The existing failure, retry and generation rules stay.
  - Older pages:
    - `read_older_saves(p, loaded, HISTORY_PAGE)`, then `merge_older` and a rebuild.
    - The view stays, and an older page arriving keeps the temporary zoom (interlude §4.6).
  - `show_new_save(save, parent)` now sets `grow = Some(save.id)` and calls `refresh()`.
  - The selection survives a reload while its save is still loaded. Otherwise the tab goes back to Now and leaves peek.
  - Sweep: `sweep_empty_timelines` runs on the worker when the tab opens and on "Check again", before the snapshot. Failures are only logged.
  - Count header:
    - `tr_plural("timeline.count", saves)` + `" · "` + `tr_plural("timeline.lines", lanes)`;
    - the pending slot is not counted;
    - with one lane, the lines part is left out.
  - Peek banner:
    - `PeekView::enter(save, before_name, line: Option<String>, lang)`.
    - It uses `tr_fmt("peek.banner_on_line", [name, date, line])` when the save's lane is not the lane holding `here`; otherwise it keeps `peek.banner`.
  - Sample repo: `make-sample-repo.ps1` keeps `feature` merged, and adds:
    - `savestates/graceful-elephant`: 4 saves, starting 30 saves before main's tip, with description `Versão com menu lateral`;
    - `savestates/quiet-river`: 2 saves, branching from graceful-elephant's second save;
    - `savestates/brave-otter`: archived, 3 saves;
    - `backup`: 10 saves before main's tip.

- [ ] **Step 1: Write the failing tests**

These go in `tabs.rs`:
- `a_changed_timelines_key_reads_the_history_again`: with key `a` shown and key `b` seen, `needs_history` is true; equal keys give false.
- `the_selection_follows_its_save_across_reloads`, through a pure `fn reselect(selected: Option<&SaveId>, data: &TimelineData) -> Selection`.
- `a_failed_history_read_is_read_again_by_the_next_refresh` and `a_failed_older_page_is_asked_for_again_once`, adapted to keys.
- `the_count_names_lines_only_when_there_are_several`, through a pure `fn count_parts(saves: usize, lanes: usize) -> (i64, Option<i64>)`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates tabs`
Expected: compile errors or failures.

- [ ] **Step 3: Implement the wiring, the widget API and the sample repo additions, and remove the old single-line paths**

The old paths are `timeline_page`, `other_timelines`, the note, `append_saved` and `prepend_older`.

- [ ] **Step 4: Run the whole workspace**

Run: `cargo test --workspace`
Expected: all pass.

- [ ] **Step 5: Smoke check**

Open the sample repo (`pwsh scripts/make-sample-repo.ps1`) and capture one screenshot (`Window::capture`). Check that:
- every lane sits at its own height (Task 7's plain glue drawing is fine here);
- each timeline's saves are in the right lane;
- peeking a save on another lane names that timeline in the banner.

- [ ] **Step 6: Commit**

`git commit -m "feat(app): the timeline reads every timeline of the project and stays in step with git"`

### Task 9: Painting lanes

**Files:**
- Modify: `crates/app/src/timeline/paint.rs`, `crates/app/src/timeline/widget.rs` (tooltip and lane-name hover only)
- Test: pure helpers in `paint.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: Tasks 7 and 8.
- Painting (spec §6.1, §6.4–§6.7; mockups B and C):
  - **Header.** The grid labels sit in the top 24 px header row, which stays put while the lanes scroll. Grid lines run through all lanes. Lanes are clipped below the header (`push_clip`).
  - **Separators.** Thin horizontal separators (`BORDER`, at the grid's alpha) at every slot boundary.
  - **Lane tint.** The current lane gets its color at 3.5 % alpha.
  - **Lane names.** Top-left of each lane, pinned to the left edge:
    - a 3 px colored dot, then the label at 10.5 px over a `BG` box at 82 % alpha;
    - drawn in the lane color, semibold when current;
    - when the current timeline is archived, ` · ` + `tr("timeline.archived_tag")` follows in `MUTED_TEXT`;
    - hovering a name shows the full name in the canvas tooltip.
  - **Lane colors.** Each lane uses its color for its line, station rings and group balls. "VOCÊ ESTÁ AQUI" (dot, ring and label) uses the color of its lane.
  - **Glow.** The wave-1 glow (same width and alpha) runs along the lane holding `here`, from its start to `here`.
  - **Branch curves.**
    - Each curve runs from the fork ball's (x, y) to (x + 72, lane y), as a cubic with controls (x + 48, fork y) and (x + 40, lane y). A straight line then goes to the lane's first save.
    - When the fork save sits in a group, the curve starts at the group ball's x.
    - A lane with `continues_left` starts at the left edge instead.
  - **Tags.** A small rounded box above the save: `MUTED_TEXT` on `SURFACE`, 10 px text, holding the label. It is drawn only when it fits before the next ball; otherwise its name joins the save's tooltip.
  - **Pending slot.** Dashed amber separators above and below, an amber tint at 4 % alpha, and `tr("timeline.pending")` in amber where a lane name goes.
  - **Agora.** Agora sits in its slot. When that is not the slot of "Você está aqui", the dashed connector runs:
    - from the ring, at (hx + 11, hy + 11);
    - as a cubic with controls (hx + 42, hy + 24) and (hx + 40, ny), to (hx + 72, ny);
    - then straight to Agora.

    When both are in one lane, it is the wave-1 straight connector.
  - **Labels.** Label rooms are computed per lane, so labels in different lanes never squeeze each other.
  - **Scroll bar.** A 3 px scroll bar (the `change_rows_widget` look) at the right edge, while the content is taller than the widget.

- [ ] **Step 1: Write the failing tests**

These go in `paint.rs`:
- `branch_curve_points`: the curve from (100, 56) to a lane at 132 gives from (100, 56), c0 (148, 56), c1 (140, 132), to (172, 132).
- `connector_into_the_pending_slot`: here at (368, 56) and Agora at (648, 132) give the specified cubic, then a line ending at x = 648 − NOW_R − 3.
- `lane_name_box_width`: the label width plus 24.
- `label_rooms_are_per_lane`: with balls at the same x in two lanes, both keep their full rooms.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates paint`
Expected: compile errors.

- [ ] **Step 3: Implement the painting as specified**

- [ ] **Step 4: Run the tests**

Run: `cargo test -p savestates`
Expected: all pass.

- [ ] **Step 5: Visual check with `Window::capture`**

Use the sample repo plus a repo with 8 timelines. Capture:
- mockup B's three-lane look;
- the pending slot after a Load (mockup C; make it with `git switch -c savestates/x <mid save>` in a terminal);
- a group sitting on a fork save;
- vertical scrolling with 8 lanes;
- an archived current timeline's tag;
- the `backup` tag;
- the week level and max zoom;
- English.

Compare them against `docs/superpowers/specs/2026-11-03-savestates-wave2-mockups.html`. Put the screenshots in `%TEMP%\twave2-9\`, view them, and remove any harness before committing.

- [ ] **Step 6: Commit**

`git commit -m "feat(app): timelines drawn as metro lanes with names, branch curves, tags and the pending lane"`

### Task 10: Load in the app

**Files:**
- Modify:
  - `crates/app/ui/project_tab.xml`:
    - `peek.load` button between `peek.everything` and `peek.back`;
    - `banner.load` button before `banner.check`.
  - `crates/app/src/peek.rs`
  - `crates/app/src/tabs.rs`
  - `crates/app/src/app.rs` (`tab.load`, `tab.load_here` actions)
  - `crates/app/src/toast.rs` (`show_text`)
  - `LEIA-ME.md`, `README.md`
- Test: unit tests in `tabs.rs` / `peek.rs` for the pure helpers

**Interfaces:**
- Consumes: Task 4 (`load`, `finish_load`, `continue_from_here`, `load_destination`, `LoadOutcome`, `LoadError`), and `App::quick_save_then` (existing).
- Produces:
  - `toast::show_text(text: &str, action: Option<Action<'_>>, seconds: f64)`: already-translated text, for messages that carry a name.
  - `fn load_button(peeked: &SaveId, here: Option<&SaveId>, read_only: bool) -> LoadButton { hidden | enabled | disabled }`: hidden when `peeked == here`.
  - `fn load_tip(timelines: &[Timeline], target: &SaveId, names: impl Fn(&Timeline) -> String) -> String`:
    - `load.tip_continue` with the destination timeline's label;
    - `load.tip_elsewhere` when the only timeline ending at the target is `elsewhere`;
    - otherwise `load.tip_start`.
- Behavior (spec §4):
  - **Clicking Carregar este save**:
    - With unsaved changes (as of the last refresh), it opens `confirm(load.unsaved_title, load.unsaved_body, [(load.save_first, 1), (common.cancel, 0)])`. Answering 1 runs `quick_save_then(tab, continuation: run the Load)`.
    - Without unsaved changes, it runs the Load.
    - The worker runs `when_normal(p, |p| load(p, &target, None))`.
  - **Ok(outcome):**
    - Run `refresh()`. When that data arrives, call `show_here_and_now()` so "Você está aqui" and Agora are both visible (spec §4.3 step 7).
    - Toast `show_text(tr_fmt(load.done or load.done_aside when set_aside is Some, [save name]), Some((common.undo, Desfazer)), UNDO_SECONDS)`.
    - Desfazer runs `load(origin.save, origin.branch)` through the same path, unsaved-changes rule included.
    - With `left_over` non-empty, show instead `confirm(load.left_over_title, load.left_over_body, [(load.retry, 1), (common.close, 0)])`. Answering 1 runs `finish_load(origin.save)` and asks again while files remain.
    - With `warning`, show `toast::show(load.hook_warning, None, NOTICE_SECONDS)` and log the details.
  - **Errors:**
    - `HasUnsavedChanges` (a race) opens the same confirm.
    - `PrefixTaken` opens `inform(load.failed_title, tr(load.prefix_taken))`.
    - `SwitchFailed { details }` opens `inform(load.failed_title, tr(load.failed), Some(details))`.
    - `NotNormal` (or `None`) refreshes; the banner says why.
  - **The banner Load button** shows only when the condition's blockers are exactly `[Detached]`. It runs `continue_from_here`, then `refresh()`.
  - **Docs:**
    - `LEIA-ME.md` gets a short PT-BR section, **Linhas do tempo**: Carregar, the new line on the next save, going back by loading a line's last save, renaming and archiving. Five bullets at most, plain words.
    - `README.md` gets one line in the feature list.

- [ ] **Step 1: Write the failing tests**

- `load_button_is_hidden_on_you_are_here_and_off_when_read_only`.
- `load_tip_says_continue_start_or_elsewhere`: three timelines set up for each case.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates load_`
Expected: compile errors.

- [ ] **Step 3: Implement the button, flows, toasts, dialogs, banner button and docs**

- [ ] **Step 4: Run the tests**

Run: `cargo test -p savestates`
Expected: all pass.

- [ ] **Step 5: Manual check in the app on the sample repo (captures)**

Check each of these:
- peek a mid-line save → Carregar → the pending slot;
- save → a new colored lane named after the save;
- Load main's last save → back, and the empty lane is gone when nothing was saved;
- unsaved changes → Salvar primeiro → the name dialog → Load continues;
- Desfazer;
- the detached banner button (make the state with `git checkout --detach` in a terminal).

- [ ] **Step 6: Commit**

`git commit -m "feat(app): Load this save: travel to any save, with undo and a save-first prompt"`

### Task 11: Lane menu, rename, archive, the archived list and deleting

**Files:**
- Create:
  - `crates/app/ui/menu_lane.xml` (`lane.rename`, `lane.archive`);
  - `crates/app/ui/dialog_rename.xml` and `crates/app/src/dialogs/rename.rs`;
  - `crates/app/ui/dialog_archived.xml` and `crates/app/src/dialogs/archived.rs`.
- Modify:
  - `crates/app/ui/menu_tabs.xml` (`tabs.archived` → `@archive.open`)
  - `crates/app/src/dialogs/recovery.rs`
  - `crates/app/src/app/recovery_glue.rs`
  - `crates/app/src/app.rs`
  - `crates/app/src/tabs.rs`
  - `crates/app/src/dialogs/mod.rs`
- Test: unit tests in `dialogs/recovery.rs`, `dialogs/archived.rs` and `tabs.rs`

**Interfaces:**
- Consumes: Task 5, Task 8's `LaneMenu` and `Rename` actions, and `TimelineWidget::lane`.
- Produces:
  - `dialogs::rename::show(window: Window, prefill: &str, on_answer: impl FnOnce(Option<String>) + 'static)`:
    - title `@rename.title`, hint `@rename.hint`;
    - an empty name is allowed and is answered as `Some("")`;
    - Esc or Cancel answers `None`.
  - `pub enum ArchivedChoice { Restore(String), Delete(String) }`
  - `dialogs::archived::show(window: Window, project: &str, rows: Vec<ArchivedTimeline>, lang: Lang, read_only: Option<&str>, on_choice: impl FnOnce(ArchivedChoice) + 'static)`:
    - Each row shows the color dot (`lane_color`), the name (`display_name`, else the git name), and `tr_plural("timeline.count", own_saves) + " · " + relative_date(last_save)`.
    - It shows `@archive.empty` when there are no rows.
  - `pub enum RecoveryItem { Work(RecoveryEntry), Timeline(DeletedTimeline) }`. `dialogs::recovery::show` takes `Vec<RecoveryItem>`, newest first, with both kinds mixed by time. A timeline row reads `tr_fmt("recovery.deleted_timeline", [name])` plus the date. `on_restore` gets the `RecoveryItem`.
  - `fn lane_refusal(lane: &LaneData, timelines: &[Timeline]) -> Option<String>`. It gives `None` when the lane may be archived; otherwise one of these texts:
    - `lane.archive_main`;
    - `lane.archive_current`;
    - `tr_fmt("lane.elsewhere", [path])`.
- Behavior:
  - **LaneMenu { lane, x, y }** opens `menu_lane.xml` with `Window::open_menu` at the pointer. Not for the pending slot, and not for the `DetachedHead` lane.
    - **Renomear** opens the rename dialog, then `rename_timeline` on the worker, then `refresh`.
    - **Arquivar** shows the `lane_refusal` text in `inform(lane.cant_archive_title, text)` when there is one. Otherwise it runs `archive_timeline` and `refresh`, then toasts `lane.archived` with Desfazer, which runs `unarchive_timeline`.
  - **Rename(lane)** (double-click on the name) opens the rename dialog directly.
  - **Linhas do tempo arquivadas…** runs `list_archived` on the worker, then opens the dialog.
    - **Restaurar** runs `unarchive_timeline` and `refresh`.
    - **Apagar…** opens `confirm_destructive(delete.title, tr_fmt(delete.body, [name, tr_plural(timeline.count, own_saves)]), (delete.confirm, 1))`. Then `delete_timeline`, and a toast `delete.done` with Desfazer, which runs `restore_deleted`.
  - **Recuperação…** lists `list_recovery` plus `list_deleted`. Restoring a timeline runs `restore_deleted` and toasts `recovery.timeline_restored`.
  - **Read-only projects:** Rename, Archive, Restore and Delete are refused, with the banner's reason (as Restore is today).

- [ ] **Step 1: Write the failing tests**

- `recovery_items_mix_both_kinds_newest_first`.
- `deleted_timeline_rows_name_the_timeline`.
- `archived_rows_show_count_and_last_save`.
- `lane_refusal_for_main_current_and_elsewhere`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p savestates dialogs`
Expected: compile errors.

- [ ] **Step 3: Implement the menus, dialogs and glue as specified**

- [ ] **Step 4: Run the whole workspace**

Run: `cargo test --workspace`
Expected: all pass, the i18n coverage test included.

- [ ] **Step 5: Manual check in the app (captures)**

Check each of these:
- rename via double-click, and via the menu with an empty name (back to the default);
- archive with Desfazer;
- the archived list → Restaurar;
- Apagar → confirm → toast Desfazer;
- delete again → Recuperação shows "Linha do tempo apagada: …" → Restaurar → it is back, archived;
- archiving the main line or the current line shows the reason.

- [ ] **Step 6: Commit**

`git commit -m "feat(app): rename, archive and delete timelines from the lane menu and the archive list"`

---

## Spec coverage map

| Spec | Task |
|---|---|
| §3.1 every branch a timeline, prefix | 1, 2, 4 |
| §3.2 main line | 3 |
| §3.3 empty (first-parent), sweep | 1, 3, 4 |
| §3.4 config keys | 1, 3, 5 |
| §4.1 Load button, tooltip, no confirmation | 10 |
| §4.2 unsaved changes block, Salvar primeiro, re-check | 4, 10 |
| §4.3 destination, ignored set aside + come back, kept ignored, switch, verify/Tentar de novo, hooks, tidy | 4, 10 |
| §4.4 Desfazer | 4, 10 |
| §4.5 saving after Load, name at first sight | 3, 8 |
| §4.6 bare save | 4, 10 |
| §5.1 git names | 4 |
| §5.2 display names | 3, 7 |
| §5.3 renaming | 5, 11 |
| §6.1 lanes, names, tint, header count | 7, 8, 9 |
| §6.2 which save where, tags, merges | 2 |
| §6.3 lane order | 2 |
| §6.4 colors, glow, curves, positions | 7, 9 |
| §6.5 Agora, pending lane | 2, 7, 9 |
| §6.6 interlude behaviors per lane, vertical room | 7, 9 |
| §6.7 note removed, peek on any lane, banner names the line | 8, 9 |
| §7.1–7.3 archive, list, delete, recovery | 5, 11 |
| §8 worktrees, live refresh, read-only, threading | 3, 4, 8, 10, 11 |
| §8.2 translations | 6 |
| §9 surviving behaviors | 7, 8, 9 (existing tests kept) |
