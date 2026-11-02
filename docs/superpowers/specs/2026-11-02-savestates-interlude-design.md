# SaveStates interlude: time grid timeline, change symbols, zip distribution

Date: 2026-11-02
Status: draft for product-owner approval
Builds on: `2026-11-01-savestates-design.md` (wave 1, merged to `main`).
This addendum **supersedes** the parts of that spec it names (mainly §6.2
"evenly spaced" stations and §7.7 packaging for now).

Markers: **[decided]** = the product owner said it; **[call]** = designer's
decision, owner may override; **[builder]** = left to the implementer.

---

## 1. Why

After using wave 1, the owner wants:
- quicker one-hand navigation of the timeline;
- the timeline to show *when* saves happened (a day grid), while keeping the
  metro balls and line that best represent saves;
- a compact change list so file names and paths get the room;
- a simple way to send the app to friends as a zip (no installer yet).

## 2. Goals, non-goals, anti-goals

Goals: §3 timeline navigation, §4 time grid with grouping and temporary zoom,
§5 change symbols, §6 zip distribution.

Non-goals: installer work (the `.iss` stays as is), installing Git for the
user (a possible later installer feature) [decided]; the deferred-findings
backlog (`docs/superpowers/plans/2026-11-01-savestates-wave1-decisions.md`)
waits [call]; timelines/branches (wave 2).

Anti-goals: losing any wave-1 timeline behavior that §4.8 lists; a timeline
that becomes an unreadable smear of overlapping balls or labels; any change
to what is saved or recovered (this is UI only, plus the build script).

---

## 3. Navigation [decided unless marked]

- **Mouse wheel zooms** around the cursor (no modifier needed). Ctrl+wheel
  keeps zooming too [call].
- **Click and drag pans.**
- Shift+wheel pans horizontally, and a touchpad's horizontal swipe (wheel x)
  pans [call].
- "Ir para Agora" and double-click on Agora stay [call].
- A click (press and release with under 4 px of movement) still selects; a
  larger movement is a pan (wave-1 rule kept).

## 4. Time grid timeline

### 4.1 Positioning [decided]
- Every **local calendar day has the same width** on screen at a given zoom,
  including days with no saves (an idle month shows a month of empty grid).
- Each save sits at its exact local time within its day:
  `x = day_index × day_width + (seconds since local midnight ÷ length of that
  day in seconds) × day_width` [call: this keeps days equal even on
  daylight-saving days].
- **Which time** [call]: position and label each save by its **commit time**
  (git committer date, `%cI`), shown in the computer's local time zone. For
  saves made in SaveStates this equals the author time; for imported history
  (rebased or cherry-picked by other tools) the commit time is the one that
  follows history order. Core's `SaveState` gains a `committed` time next to
  the existing author `time` (same parsing fallback rules).
- **History order wins** [call]: x never decreases along history. A save
  whose commit time is earlier than the previous (older) save's is drawn just
  to the right of it (a minimal gap), never to its left. Saves with an
  unreadable date (core's 1970 fallback) are placed the same way and are left
  out of range, paging and "fit" calculations. A save dated in the future is
  drawn at the current time.
- The metro line connects saves in history order (first-parent), as now; the
  cyan glowing line and ball styles stay.

### 4.2 Background grid [decided; levels are a call]
- Thin vertical lines at day boundaries with day labels at the top
  ("dom 26 out", "seg 27", … "hoje"; today's column softly highlighted).
- The grid adapts to zoom [call]:
  - zoomed in so an hour is wide enough: hour lines and labels ("13h",
    "14h"), with the day named on the first one ("sex 31 · 13h");
  - normal: days;
  - zoomed out so days get narrow: weeks ("semana de 28 set", then "5 out",
    …);
  - further out: months ("out 2026").
  Exact thresholds are [builder]; aim for labels never overlapping.
- All grid labels come from the translation table / i18n date helpers, PT-BR
  first (weekday and month abbreviations need new localized helpers next to
  `relative_date`) [call].

### 4.3 Zoom range [call]
From about **one minute ≈ 40 px** (zoomed in, so saves made a minute or more
apart can always be told apart) to roughly two years fitting the widget
(zoomed out). The grid gains a minutes level ("13:05", "13:10", …) above
hours. Default zoom on first open: one day ≈ 140 px. Numbers are [builder]
within that intent.

### 4.4 Agora [call, approved in conversation]
- "Agora" sits at the **current time** on the grid, dashed amber like today,
  with the dashed line from "Você está aqui" to it — even across a long gap.
- **Minimum gap** [call]: right after a save, Agora's real time is on top of
  "Você está aqui". Agora is drawn at its time **or** at a minimum distance
  (about 56 px) to the right of "Você está aqui", whichever is further right,
  so the two balls and their labels ("VOCÊ ESTÁ AQUI" above the save, the
  save's name below it, "Agora" below Agora) never collide. A click selects
  the nearest of the two.
- Its position follows the clock (update at least once a minute and on every
  refresh).
- An empty project (no saves) shows only Agora at the current time.

### 4.5 Grouping [decided; threshold is a call]
- Saves whose balls would overlap at the current zoom merge into one
  **larger ball with the count** inside (radius grows gently with the count,
  capped) [call for the sizing].
- Merge rule [call]: walking saves in history order, a save joins the
  current group when its screen x is within about **two ball widths of the
  group's first member** (not its previous one, so a long chain of saves
  becomes several neighbouring groups instead of one huge one). Recomputed
  whenever the view changes.
- A group is drawn at the **middle of its span**; group balls are capped in
  size so neighbouring groups never overlap [call].
- **Pinned balls** ("Você está aqui", the selected save; see below) split the
  walk: groups form on each side of them, and a group that would overlap a
  pinned ball is nudged sideways away from it; the pinned ball is drawn on
  top as a last resort [call].
- Group label under the ball: "n saves". Hover tooltip:
  - all in one day: "15 saves em 31 de out.";
  - same week: "9 saves na semana de 26 out";
  - same month: "22 saves em outubro";
  - otherwise: "n saves entre 28 set e 12 out" [call].
  Then "clique para aproximar" on a second line.
- **Never grouped** [call, approved]: "Você está aqui" (the latest save) and
  the currently selected/peeked save always keep their own ball; grouping
  happens around them.

### 4.6 Temporary zoom [decided]
- **Clicking a group** animates the view so that group's time span fills
  about the middle 60% of the widget (capped at max zoom). This is a
  *temporary* zoom.
- **Groups that can't split** [call]: if the group is already at max zoom
  (saves less than about a minute apart, or with identical times), clicking
  it opens a small list of its saves (name + time) right under the ball
  instead; picking one peeks it, Esc or clicking elsewhere closes the list.
  Its tooltip then says "clique para ver a lista" instead of "clique para
  aproximar". Every save stays reachable.
- While temporary, a small pill shows "Aproximado · Esc para voltar"
  [call, approved].
- **Esc** animates back to the exact view from before the temporary zoom.
- **Any user pan or zoom** (drag, wheel) "accepts" the temporary zoom: the
  pill disappears and Esc no longer goes back.
- Clicking another group while already temporary zooms to it but keeps the
  *original* return point (Esc goes back to where the user started) [call].
- Clicking a single save still **peeks** (wave-1 behavior), and does not end
  the temporary zoom [call].
- **What else accepts or keeps it** [call]: any view change that isn't Esc
  or a group click *accepts* (ends) the temporary zoom — drag, wheel,
  "Ir para Agora", double-click on Agora, and the auto-scroll after a new
  save. Things that don't move the view keep it: peeking a save,
  `Select(Now)`/"Voltar para Agora", a refresh, an older-history page
  arriving (the view shifts to keep the same saves in place), and switching
  tabs (each tab keeps its own state).
- Esc must work while the mouse is over the timeline or the timeline has
  focus; it must not interfere with Esc in dialogs [builder for the
  mechanism].

### 4.7 Labels and detail [call]
- Under a single ball: name, then time ("15:02") or date+time when the grid
  is at week/month level; names truncated to fit the space to the next ball
  (wave-1 fitting logic); hidden when there is no room.
- Hover tooltip on a single save: full name, date and time, file count (as
  now).

### 4.8 Wave-1 behaviors that must survive
"Você está aqui" ring and label; dashed amber Agora with the unsaved count;
selection halo; peek on click and `Select(Now)` to leave; paging older
history as the user pans toward the past (now judged in time, not station
count); the grow-in animation for a new save; the "outras linhas do tempo"
note; live language switch; the read-only conditions; hover tooltips.

### 4.9 Persistence [call]
`settings.timeline_zoom` keeps one value per project, but wave-1 values mean
something else: store the new zoom under a new key (or clear old values) so
an old value is never misread.

---

## 5. Change list symbols [decided unless marked]

- Applies to **both** change lists: "Agora" (unsaved changes) and peek's
  "O que mudou" [call, approved].
- Each row: a colored symbol, then the **file name**, then its **folder** in
  muted text aligned right.
  | Kind | Symbol | Color | Tooltip (PT-BR / EN) |
  |---|---|---|---|
  | New | `+` | green (theme ADDED) | Novo / New |
  | Deleted | `−` | red (theme REMOVED) | Apagado / Deleted |
  | Changed | `M` | amber (theme UNSAVED) | Alterado / Changed |
  | Renamed | `R` | cyan [call] | Renomeado de {0} / Renamed from {0} [call] |
- Hovering the symbol shows the tooltip. Long folders are shortened in the
  middle ("src/…/componentes/") and hovering the folder shows it in full
  [call, approved].
- Must keep: row selection (first row selected by default), right-click menu
  with "Descartar alterações deste arquivo" on the Agora list (right-click
  selects the row first), read-only disabling, scrolling, and staying fast
  with thousands of rows (an un-ignored `node_modules`).
- Raven's list view only shows plain text and only shows a tooltip when the
  pointer enters a different widget, so this likely needs a small
  custom-painted list (like the comparison view and the timeline) [builder].

## 6. Distribution as a zip [decided: zip, no installer for now]

- `python scripts/build.py --zip` builds the app and writes
  `target/runnables/SaveStates-<version>-windows-x86_64.zip` [call] with one
  top-level folder `SaveStates/` containing: `savestates.exe`,
  `vcruntime140.dll`, `RAVEN-LICENSE.txt`, `RAVEN-NOTICE.txt`, `LEIA-ME.md`
  [call: no spike exe in the zip].
- `LEIA-ME.md` (PT-BR, for non-technical friends) switches from the
  installer to the zip [call, approved]:
  1. extract the zip anywhere (e.g. Documentos) and open `savestates.exe`;
  2. SmartScreen: "O Windows protegeu o computador" → **Mais informações** →
     **Executar assim mesmo**;
  3. "Instale o Git": link `https://git-scm.com/download/win`, accept the
     default options; mention the app's own "Git não encontrado" screen;
  4. optional Claude Code for AI names; the log file note stays.
- `README.md` (EN) documents `--zip`. The installer section stays but is
  marked "not used yet".

## 7. Left to the builder
Grid level thresholds, zoom numbers within §4.3, group ball sizing,
animation durations/easing, how Esc reaches the timeline, the custom list
widget's structure, and test strategy details — as long as the timeline
math, grouping, temporary-zoom state and row model are pure, unit-tested
code like wave 1's `timeline/model.rs`.
