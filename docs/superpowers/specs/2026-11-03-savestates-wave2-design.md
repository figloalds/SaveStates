# SaveStates wave 2: timelines

Date: 2026-11-03
Status: builder-checked draft, awaiting product-owner approval
Builds on: `2026-11-01-savestates-design.md` (wave 1) and
`2026-11-02-savestates-interlude-design.md` (time grid timeline), both merged to `main`.
This document **supersedes** the parts of those it names: wave-1 §6.2's "only the current
timeline" and its "other timelines" note, wave-1 §6.8.4's handling of a detached HEAD, and
wave-1 §8's sketch of Load.

Markers: **[decided]** = the product owner said it; **[call]** = designer's decision, the owner
may override it; **[builder]** = left to the implementer.

---

## 1. Intent

A **timeline** is git's branch, told as time travel [decided]. Going back to an old save and
saving something new there creates an **alternate timeline**, as in popular fiction. Nobody
creates, names or deletes a branch by hand. The app does the git work, automatically and
deterministically [decided].

Wave 1 already drew one timeline: the current branch. Wave 2 adds:
- **Load**: move the project to any save (§4);
- **automatic timelines**: a new app branch is made only when Load lands in the middle of a
  line, kept when it gets a save, and removed when it is left empty (§3, §4);
- **every local branch drawn as a lane** on the interlude's day grid, metro-colored (§6);
- **names** friends can read and change (§5);
- **archiving** timelines and deleting them from the archive, recoverably (§7).

### Why
Friends vibe-code with AI tools. When the AI wrecks something, wave 1 can only copy the past
into the present (Bring back). Wave 2 lets them stand in the past and try another path while
the first one stays intact and visible.

### What success looks like [call]
A friend peeks at yesterday's save, clicks **Carregar este save**, makes changes, saves, and
sees a new colored line branch off the old one. They can go back to the old line by loading
its last save. A developer opening the project in another git tool finds ordinary branches
named `savestates/<words>` and no stray empty ones.

---

## 2. Goals, non-goals, anti-goals

Goals: §3–§8.

Non-goals:
- Joining timelines (merge/rebase): wave 3 [decided].
- Remotes: `origin/…` branches are not drawn, nothing is pushed or pulled [call].
- A "new timeline" button, or any hand-made branch: timelines appear only through Load and
  saving [decided].
- Choosing a timeline's color, reordering lanes by hand, tags, moving saves between timelines,
  stash [call].
- The deferred-findings backlogs of wave 1 and the interlude [call].

Anti-goals (it fails, even if it works, when...):
- ...the user **loses work** through Load, archive or delete. Load never runs with unsaved
  changes; ignored files git would overwrite are set aside first; a deleted timeline stays
  recoverable for 30 days [decided + call].
- ...the app **changes a branch it didn't make** on its own: no automatic delete, rename or
  move of any branch outside its prefix [decided]. Only an explicit user action (Apagar, §7)
  deletes one.
- ...the app **leaves stray branches**: an app branch with no save of its own never outlives
  the moment the user leaves it [decided].
- ...the timeline becomes an **unreadable tangle**: lines cross needlessly or labels overlap.
- ...the user sees **git words**. A branch's own name (`develop`) is a name, not jargon, and
  may show; "branch", "checkout", "HEAD" may not.

---

## 3. Timelines and the git underneath

### 3.1 Every local branch is a timeline [decided]
- Every local branch (`refs/heads/*`) is a timeline and works the same way, whoever made it.
  The branch the project is on is the **current timeline**; Quick Save adds to it, as in wave 1.
- **App branches** carry the prefix `savestates/` (git name `savestates/<words>`, §5.1). The
  prefix is the only thing that sets them apart: the app creates them automatically and
  deletes them automatically when empty. It never automatically deletes, renames or moves a
  branch without the prefix.
- A branch named `savestates/…` by someone else is treated as an app branch. Documented, not
  guarded against [call].

### 3.2 The main line [call]
- The **main line** is the branch the project was on the first time wave 2 opened it, usually
  `main` or `master`. It is remembered in the project's git config as
  `savestates.main-line = <branch>` (local config, travels with the folder).
- When that first open finds the project on an app branch (or the remembered branch no longer
  exists), the main line falls back to `main`, then `master`, then the branch with the longest
  history [builder may refine the last step].
- The main line is drawn on top, in cyan. It cannot be archived or deleted (§7). It can be
  renamed (§5.3).

### 3.3 "Empty" and the sweep [decided; definition is a call]
- An app branch is **empty** when it has no save of its own: its last save is also on the
  **first-parent line** of another local branch (any, archived or not). First-parent, not git's
  usual "contains": a timeline that another tool merged with `git merge` (its last save reached
  only through the merge's second parent) is **not** empty, so it is kept and still drawn as a
  lane. This is the same test §6.2 uses for "has a lane", and it can only delete less than git's
  usual test would [call].
- An empty app branch is deleted:
  - when the user Loads somewhere else and leaves it (§4.3, step 6);
  - by a **sweep** each time the project opens (and on "Check again"), for every empty app
    branch that is not checked out in any working folder (the project's or another git
    worktree's). This covers a crash between Load and the first save.
- A commit made on an app branch by another tool (Claude Code committing from a terminal)
  makes it non-empty, so it is kept. This is why the branch is created at Load time instead of
  leaving git on a bare save [decided].

### 3.4 Where the app keeps its notes [call, approved in conversation]
All per-timeline data lives in the branch's own section of the project's `.git/config`. Git
removes the section when the branch is deleted and moves it when a branch is renamed; other
tools ignore the keys they don't know.

| Key | Meaning |
|---|---|
| `branch.<name>.description` | The display name (§5.2). A standard git field. |
| `branch.<name>.savestates-archived` | `true` when archived (§7). |
| `savestates.main-line` | The main line's branch name (§3.2). |

Besides these keys, the app writes only wave 1's `.gitignore`, the private refs under
`refs/savestates/` (recovery, and deleted timelines in §7.3), and its own commented block in
`.git/info/exclude` (§4.3 step 3). None of it is ever part of a save.

---

## 4. Load [decided unless marked]

### 4.1 Where it lives
- Clicking a save still only **peeks** (wave 1). The peek banner gains **Carregar este save**
  between "Trazer tudo de volta" and "Voltar para Agora" [call, approved via mockup].
- It is hidden when the peeked save is "Você está aqui". It is disabled, with the read-only
  explanation, in the wave-1 read-only states.
- Its tooltip says what will happen [call]: "Continua a linha do tempo {nome}" when the save is
  the last one of a timeline, otherwise "Seus próximos saves vão começar uma nova linha do
  tempo a partir daqui".
- No confirmation dialog: Load loses nothing and can be undone (§4.4) [call].

### 4.2 Unsaved changes block Load [decided]
- Load never carries, stashes or discards unsaved changes. With any unsaved change (tracked
  changes and new, not-ignored files), Load shows a message instead:
  "Você tem alterações não salvas. Salve antes de carregar outro save."
  Buttons: **Salvar primeiro** (default) and **Cancelar** [call].
- **Salvar primeiro** runs the normal Quick Save flow (safety check, name, hooks). When the save
  succeeds, Load continues; if the save is cancelled or fails, Load stops [call].
- Throw away is not offered here; the existing Throw away button covers that [call].
- Like Quick Save, Load re-checks for unsaved changes on the worker immediately before acting,
  so changes written a moment earlier by an AI tool can't slip through [call].

### 4.3 What Load does [decided; ordering and details are calls]
Given the target save S:
1. Re-check: repository condition is normal and the folder has no unsaved changes (§4.2).
2. **Decide the destination.** If a visible (not archived) timeline other than the current one
   has S as its last save, and its branch is not checked out in another git worktree, the
   destination is that branch: no new branch [decided]. When several qualify: the lane S is
   drawn in, then the main line, then app branches, then the rest by name [call]. Otherwise the
   destination is a new app branch at S.
3. **Ignored files in the way** [call]. Git silently overwrites ignored files that sit where the
   destination save has a tracked file (an ignored `.env` today, a saved `.env` in the past),
   and removes them again on the way back. Before switching, every ignored file the switch
   would overwrite or remove is snapshotted into the Recovery area (the mechanism Bring back
   already uses for "ignored files in the way") and the toast mentions it.
   **They come back by themselves** [call]: such a snapshot is marked as "set aside by Load".
   After every Load (Desfazer included), each marked snapshot whose paths are all free again
   (not tracked by the new save and absent from the folder) is restored and dropped. So Load
   into the past and back leaves the friend's real `.env` exactly where it was. A snapshot that
   can't come back yet stays in Recuperação like any other (30 days) [mechanism: builder].
   **What was ignored stays ignored** [decided]. Older saves often have a shorter `.gitignore`
   (wave 1's "Leave these out" added `node_modules/` in save 5; the friend loads save 3). Files
   that are on disk and ignored before the switch, and would no longer be ignored after it, are
   added to the project's private exclude list `.git/info/exclude` (this computer only, never
   saved, respected by every git tool). Exactly those paths are added, anchored and literal
   (`/node_modules/`, `/.env`), under a comment that says why:
   `# SaveStates: kept ignored after loading an older save`. So Agora never fills with files
   the friend didn't make, the next Load (and Desfazer) is never blocked by them, and Throw
   away can't touch them. The entries stay: the friend already chose to leave those paths out,
   so they stay out on every timeline. Whether the set is predicted before the switch or found
   right after it is [builder], as long as the friend never sees those files as changes.
4. **Switch**: `git switch <branch>`, or `git switch -c savestates/<words> <S>` for a new app
   branch. Project hooks (`post-checkout`) run normally; a hook failure after a completed
   switch is reported as a warning, not as a failed Load [call].
5. **Verify**: read status again. If files still differ from S (a file locked by another program
   on Windows stopped git part-way), say so plainly ("Alguns arquivos não puderam ser
   atualizados porque outro programa está usando eles. Feche esse programa e clique em Tentar
   de novo.") with **Tentar de novo**, which finishes the switch for the tracked files.
   Nothing is lost either way: everything was saved before Load [call; mechanism: builder].
6. **Tidy**: if the timeline just left was an app branch that is now empty (§3.3), delete it.
7. Refresh the timeline: "Você está aqui" moves to S, the selection returns to Agora, and the
   view scrolls so both are visible.

### 4.4 Undo [call]
After Load, a toast "Você carregou {save}" with **Desfazer** for about 10 seconds (as Throw
away's toast). Desfazer Loads back to the save the user came from, through the same §4.3 rules
(so it returns to the same timeline when that timeline still exists, and is refused under the
same unsaved-changes rule).

### 4.5 Saving after Load
- A Quick Save on a new (empty) app branch is an ordinary save. The branch now has a save of its
  own, so its lane appears (with the wave-1 grow-in animation for the new save) [call].
- At that first save the app writes the display name (§5.2). If another tool made the first
  save, the app writes the name the first time it sees the branch with a save of its own.

### 4.6 A project left on a bare save by another program [call, approved in conversation]
Wave 1 opens a detached HEAD read-only. That stays, and the read-only banner gains **Carregar
este save**. It follows §4.3 step 2: when a visible timeline's last save is the current save
(another tool ran `git checkout origin/main` while `main` is at the same save), it switches to
that branch; otherwise it creates an app branch at the current save (`git switch -c`). Neither
changes any file, so unsaved changes are fine here and are carried along. The project then
continues normally. No other read-only state changes.

---

## 5. Names

### 5.1 Git names [decided]
- App branches are named `savestates/<adjective>-<noun>` from a built-in English word list
  (`savestates/graceful-elephant`), picked at Load time and **never renamed** by the app. On a
  collision, pick again; after a few tries, add a number [builder for the list and retries].
- If a branch literally named `savestates` exists, git can't create `savestates/…` branches;
  Load then explains it can't start a new timeline in this project [call; rare].

### 5.2 Display names [decided]
- The name friends see is stored in `branch.<name>.description` (§3.4).
- An app timeline's default display name is the **full name of its first save**, copied when
  that save is made (accents and spaces intact; never turned into a branch name).
- A branch the app didn't make shows its git name (`develop`) until renamed.
- The main line shows "Linha principal" / "Main line" until renamed, whatever git calls it.
- A timeline with no save of its own yet (just loaded) has no lane and no name; its pending
  lane reads "Nova linha do tempo · começa no seu próximo save" (§6.5).

### 5.3 Renaming [decided; gestures are a call]
- Double-click a lane's name, or right-click it → **Renomear**. A small dialog like the save
  name dialog, prefilled with the current name; Enter saves, Esc cancels.
- Renaming only changes the display name (writes `description`), so it works the same on every
  timeline, including the main line and branches the app didn't make. The git branch is never
  renamed.
- Saving an empty name restores the default name [call].

---

## 6. Drawing timelines (the lanes) [decided via mockups A/B/C; details as marked]

The mockups shown to the owner are in `2026-11-03-savestates-wave2-mockups.html` next to this
document (open it in a browser). Option **B** (metro colors) was chosen; **C** shows Load and
the pending lane.

### 6.1 Lanes
- The interlude's day grid stays: vertical day/hour/week/month lines and labels, shared by all
  lanes. **Thin horizontal lines separate lanes** [decided], one lane per visible timeline.
- Each lane holds its timeline's line and balls, the names and times under them, and room above
  for the "VOCÊ ESTÁ AQUI" label (about 76 px per lane in the mockup) [builder].
- The lane's **name** sits at its top-left and stays pinned to the left edge while panning, over
  a soft background so it stays readable above balls passing under it [call, mockup]. Hovering
  it shows the full name; a right-click opens Renomear / Arquivar linha do tempo (§7).
- The lane you are on is softly tinted with its color [call, mockup].
- The header shows "11 saves · 3 linhas" (saves counted once) [call].

### 6.2 Which save is drawn where [call]
- Each save is drawn exactly once, in one lane. A lane holds the saves of its timeline that no
  lane above it already holds, following first parents.
- A timeline whose line contains another timeline's whole line continues from it instead of
  swallowing it (a `feature` branched from `develop` forks off `develop`'s lane).
- The assignment depends only on git data (no stored layout), so it is the same after a
  restart [builder for the algorithm].
- "A save of its own" uses §3.3's first-parent test, so the lane rule and the empty rule
  always agree.
- A branch with no save of its own that isn't the current timeline (a backup branch pointing
  into another line, a branch fast-forwarded by another tool) gets no lane: a small tag with its
  name on the save it points at [call; tag style: builder]. Loading that save follows §4.3
  (that branch's last save is S, so Load continues it).
- Saves reachable only through a merge's second parent are not drawn, as in wave 1; a merge
  save may carry a small icon [builder]. A connector to a visible merged lane is optional
  [builder].

### 6.3 Lane order [call, approved via mockup notes]
- The main line is the top lane.
- A timeline's lane sits **right under the lane it branched from**. Several timelines branching
  from the same lane are ordered newest first (the newest closest to its parent), each followed
  by the timelines that branch from it. This keeps branch curves short and rarely crossing.
- "Newest" is the commit time of the timeline's first own save; the pending lane (§6.5) counts
  as the newest.

### 6.4 Lines, branch curves and colors
- **Metro colors** [decided, mockup B]: every timeline keeps its own color. The main line is
  cyan (`#38bdf8`). Other timelines take colors from a palette that avoids green, red and amber
  (they mean added, removed and unsaved): violet `#a78bfa`, pink `#f472b6` and a few more of
  that family [builder for the rest of the palette].
- A timeline's color is **stable**: the same on every open, unaffected by lanes moving. It is
  derived from the git branch name (a stable hash into the palette), so nothing is stored and
  nothing is written just by looking [call; the builder may store a color at creation for app
  timelines if that avoids neighbours sharing a color].
- The color applies to the line, the rings of its balls, its "VOCÊ ESTÁ AQUI" dot and label,
  its lane's name and tint, and its group balls.
- The timeline you are on glows (the wave-1 glow, up to "Você está aqui"); the others are drawn
  in full color without the glow [decided, mockup B].
- A branch line starts on its parent lane at the save it branched from, runs along the parent
  line just past that save's label, then curves down into its lane and continues horizontally
  to its first save [call, mockup]. The curve must not cut through labels.
- Positions follow the interlude: each save at its commit time on the shared axis, and "history
  order wins" along each line, including from the branch point to the timeline's first save.

### 6.5 Agora and the pending lane [decided, mockup C]
- "Agora" sits on the current timeline's lane at the current time (interlude §4.4), dashed
  amber, with the dashed connector from "Você está aqui".
- When "Você está aqui" is not the last save of its line (right after a Load into the middle),
  a **pending lane** appears right under that line: dashed amber separators, a faint amber tint,
  the name "Nova linha do tempo · começa no seu próximo save" in amber italic. Agora sits in it,
  and the dashed connector bends from "Você está aqui" down into it. The first save turns it
  into a real lane.
- The same pending lane shows whenever the current branch has no save of its own and is not the
  last save of a visible line [call].

### 6.6 Interlude behaviors per lane [call]
- Grouping (interlude §4.5) runs per lane. Pinned balls: "Você está aqui" and the selected
  save, as now. A branch curve starts at whatever ball (single or group) holds its branch
  point.
- Temporary zoom, the unsplittable-group list, Esc, wheel/drag navigation, labels and fitting,
  tooltips, the grow-in animation and paging toward the past keep working, across lanes.
- **Vertical room**: when lanes don't fit the band, the timeline scrolls vertically. Drag pans
  in both directions; a thin scroll bar appears at the right edge; the plain wheel still zooms
  [call]. The band's height stays user-resizable (wave-1 splitter).
- "Ir para Agora" and double-click on Agora also scroll vertically so Agora's lane is visible.
- Paging older history is judged in time across all lanes [builder for the mechanism].

### 6.7 What else changes on screen
- The wave-1 note "Este projeto tem outras linhas do tempo…" is removed: they are drawn now.
- Peeking a save on any lane works as in wave 1 (What changed / All files / Bring back),
  so bringing a file over from another timeline is already possible [call].
- The peek banner names the timeline when it is not the current one:
  "Espiando {save} · {data} · em {linha}" [call].

---

## 7. Archive and delete [decided; details are calls approved in conversation]

### 7.1 Archiving
- Right-click a lane's name → **Arquivar linha do tempo**. Not offered for the main line, or for
  the timeline you are on (the menu item is disabled with "Carregue outro save antes de
  arquivar esta linha do tempo").
- Archiving writes `branch.<name>.savestates-archived = true`. The branch is untouched, so other
  git tools still see it normally; only SaveStates hides it.
- An archived timeline's lane disappears. Saves it shares with a visible timeline (the part
  before they split) stay where they are, drawn by that timeline (§6.2).
- If another tool switches the project onto an archived branch, it is drawn while current, with
  a small "arquivada" tag on its lane name.

### 7.2 The archive list
- The project's right-click menu (where **Recuperação…** is) gains **Linhas do tempo
  arquivadas…**. A dialog lists archived timelines: color dot, name, number of saves of its
  own, date of its last save, and **Restaurar** / **Apagar…** per row. Empty state: "Nenhuma
  linha do tempo arquivada."
- **Restaurar** removes the archived flag; the lane comes back.

### 7.3 Deleting
- **Apagar…** asks first: "Apagar '{nome}' e seus {n} saves?" with **Apagar** and **Cancelar**.
- Delete is real: the branch is removed (`git branch -D`), from SaveStates and from every git
  tool [decided]. It works on branches the app didn't make too, because the user asked
  explicitly [call].
- **Recoverable for 30 days** [call]: before deleting, the app keeps the timeline's last save on
  a private ref under `refs/savestates/deleted/`, with what it needs to restore the timeline
  (git name, display name). The Recovery list (**Recuperação…**) shows it as
  "Linha do tempo apagada: {nome}"; **Restaurar** recreates the branch (its old git name, or a
  free variant), with its display name, **archived**. Deleted timelines are pruned with the
  rest of Recovery after 30 days. A toast with **Desfazer** follows the delete, as for Throw
  away.
- A branch checked out in another git worktree can't be archived or deleted; the app says
  "Esta linha do tempo está aberta em outra pasta ({pasta})." [call]

---

## 8. Other rules

- **Worktrees** [call]: branches checked out in other git worktrees are drawn like any other.
  Loading the last save of such a branch starts a new app timeline from it (git can't check
  out one branch twice) and the tooltip says so. The sweep never deletes a branch checked out
  anywhere.
- **Live refresh**: branches created, moved or deleted by other tools redraw the lanes (the
  watcher already reacts to ref changes).
- **Read-only states** (wave-1 §6.8.4): Load, archive, delete and rename are disabled with the
  same explanation, except §4.6.
- **Threading and queue** (wave-1 §7.2–7.3): Load, the sweep, archive, delete and restore run on
  the worker through the per-project queue, never on the UI thread.
- **Settings**: no new settings. The timeline zoom per project is unchanged.

### 8.1 Concept → git mapping (wave 2 additions)
| User action | Git underneath |
|---|---|
| Load the last save of a timeline | `git switch <branch>` |
| Load any other save | `git switch -c savestates/<words> <sha>` |
| Keep ignored files ignored across Load | append anchored paths to `.git/info/exclude` |
| Ignored files in the way of Load | snapshot to a Recovery ref marked "set aside by Load"; restored after a later Load frees their paths |
| Leave an empty app timeline | `git branch -D savestates/<words>` (after the switch) |
| Sweep at open | find app branches whose tip is contained in another local branch, not checked out anywhere; delete them |
| Read timelines | `git for-each-ref refs/heads` + first-parent history of each (paged) |
| Rename | `git config branch.<name>.description <text>` |
| Archive / restore | `git config [--unset] branch.<name>.savestates-archived` |
| Delete | save the tip under `refs/savestates/deleted/…`, then `git branch -D <name>` |
| Restore a deleted timeline | `git branch <name> <sha>` + description + archived flag |
| Continue from a bare save (§4.6) | `git switch -c savestates/<words>` |

### 8.2 Translation
All new texts go through the translation table, PT-BR first. The PT-BR strings quoted in this
document are the designer's proposals for the owner to correct [call]. Glossary additions:

| Concept | PT-BR | English |
|---|---|---|
| Load | Carregar este save | Load this save |
| main line | Linha principal | Main line |
| new timeline (pending) | Nova linha do tempo · começa no seu próximo save | New timeline · starts with your next save |
| archive | Arquivar linha do tempo | Archive timeline |
| archived timelines | Linhas do tempo arquivadas | Archived timelines |
| archived (tag) | arquivada | archived |
| delete | Apagar | Delete |
| rename | Renomear | Rename |
| save first | Salvar primeiro | Save first |

---

## 9. Wave-1 and interlude behaviors that must survive
Everything in interlude §4.8 and §5 (change symbols), plus: Quick Save on the current timeline,
peek and Bring back from any save, Throw away and Recovery, the safety checks, AI naming, hooks,
live refresh, read-only detection, Esc and temporary zoom, and the first view that shows the
last save (now: "Você está aqui" and Agora).

## 10. Left to the builder
The lane-assignment algorithm (§6.2) and paging across lanes; lane height, the curve's exact
geometry, label fitting around curves; the color palette beyond the first three and the hash;
the word lists for git names; the mechanism for §4.3 step 5 ("Tentar de novo"); how deleted
timelines record their names under `refs/savestates/deleted/`; tag style for lane-less
branches; merge icons; test strategy, as long as lane assignment, Load's destination rule, the
empty rule and the sweep are covered by tests against real temporary repositories (git/core)
and pure unit tests (app timeline model), as in wave 1 and the interlude.
