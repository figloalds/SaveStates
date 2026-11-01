# SaveStates: design (wave 1)

Date: 2026-11-01
Status: builder-checked draft, awaiting product-owner approval

Marking used in this document:
- **[decided]**: the product owner said this.
- **[call]**: a decision the designer made; the product owner can override it.
- **[builder]**: left to whoever builds it.

---

## 1. Intent

SaveStates is a desktop git client for people who don't know git. It
reframes version control in video-game and time-travel terms:

- A **commit** is a **save state**, a snapshot you can come back to.
- A **branch** is a **timeline** (wave 2).
- History is drawn **left to right**, like a metro map. It does not run top to
  bottom like classic git tools.

Underneath, it is real, standard git. A developer can open any project
SaveStates touched with normal git tools and find a normal repository. The
user never sees git words (commit, stage, index, HEAD, hash, branch,
rebase, merge, checkout).

### Who it's for [decided]
The product owner's friends: native **Brazilian Portuguese** speakers, office
workers, not technical, with some video-game experience (hence "save
state"). Right now they are "vibe coding". Their projects hold Markdown
and source code, much of it written by AI tools such as Claude Code. Office
documents (Word, Excel) are not a focus. The app runs on their **own
personal Windows computers**, not company-managed machines [decided].

### Why
They need robust version control: the ability to go back when the AI makes a
mess. Today the only route is git's vocabulary and mental model, which they
don't have. Game saves are a model they already understand.

### What success looks like [call]
A friend who has never heard of git, given the app and a project folder,
can save their work, see what changed, and get an old version of a file
back, without asking anyone. When they later meet git elsewhere, the
history they made is clean and linear.

---

## 2. Goals, non-goals, anti-goals

### Goals (wave 1)
- Open one or more projects, each in its own tab, in a GitKraken-like window [decided].
- **Quick Save**: save everything that changed in one action ("add all + commit") [decided].
- Save naming: ask the user (default), or have Claude (Haiku) write the name [decided].
- See what changed since the last save, and what changed in any past save (diffs) [decided].
- A left-to-right, zoomable, draggable metro-line timeline of save states [decided].
- Get the past back without rewriting history (§6.6) [call].
- UI in Brazilian Portuguese first, English second [decided: translate; call: PT-BR primary].

### Non-goals (wave 1)
- Timelines/branches (wave 2) and joining timelines/merges (wave 3) [decided].
- Remotes, push, pull, GitHub sign-in (a later wave) [decided].
- Picking individual files or lines to save (no staging area, ever, in the main flow) [call].
- Office-document diffs [decided by audience].
- macOS and Linux releases. The code must not be Windows-only, but only Windows is built, tested and shipped in wave 1 [call].

### Anti-goals: it fails, even if it works, when...
- ...the user **loses work** because of anything the app did. Every
  destructive-looking action can be undone or recovered [call].
- ...the user sees **git jargon or raw git error text**. Errors are
  translated into plain language. Raw output can sit behind a "details"
  expander for whoever is helping them [call].
- ...the app **produces messy history**: merge bubbles, stray branches,
  rewritten published history. History is linear by default, and later waves
  prefer rebase + fast-forward when joining timelines [decided].
- ...the app **saves junk or secrets** (`node_modules`, `.env`, private keys) without warning [call].
- ...the app **makes a broken repository worse**. When it doesn't understand a repo's state, it refuses to act and explains [call].

---

## 3. Vocabulary and glossary

Every user-facing string goes through translation keys (§7.6). The words
below are the product's vocabulary. The PT-BR column is **the designer's
proposal, for the product owner to correct** [call]. Where Brazilian game
localizations have a standard term, it is used.

| Concept (git) | English UI | PT-BR UI (proposal) |
|---|---|---|
| commit | save state (short: "save") | save (pl. saves). Alternative: "ponto de salvamento" |
| add -A + commit | Quick Save | Salvamento rápido |
| commit message | save name | nome do save |
| branch (wave 2) | timeline | linha do tempo |
| HEAD / tip | You are here | Você está aqui |
| working-tree changes | Now · unsaved changes | Agora · alterações não salvas |
| changed files | Changes | Alterações |
| diff, old side / new side | Before / After | Antes / Depois |
| viewing an old commit | Peeking at a save | Espiando um save |
| restore from an old commit | Bring back | Trazer de volta |
| discard working changes | Throw away changes | Descartar alterações |
| git init | Start saving this folder | Ativar saves nesta pasta |
| open repo | Open project | Abrir projeto |
| checkout old commit (wave 2) | Load | Carregar |
| merge/rebase timelines (wave 3) | Join timelines | Unir linhas do tempo |
| commit-message mode: prompt | Ask me for a name | Perguntar o nome |
| commit-message mode: AI | Let AI name it | Deixar a IA dar o nome |

---

## 4. Waves

| Wave | Scope |
|---|---|
| **1 (this document)** | Projects, one timeline per project, Quick Save, changes, diffs, peek, bring back, throw away, safety rails, PT-BR/EN. |
| 2 | Timelines. Clicking a station and choosing **Load** travels there. Saving from a point that isn't the end of a line **automatically starts a new timeline** (a new parallel metro line). The user never creates branches by hand [decided]. |
| 3 | Joining timelines. The default strategy replays one timeline's saves on top of the other (rebase), then fast-forwards, so history stays linear [decided]. Conflicts are explained in plain language. |
| Later | Remotes: push/pull (pull prefers rebase), sync status, sign-in [decided: deferred]. |

### Wave-1 constraints that protect waves 2 and 3 [call]
- The domain model has a `Timeline` type from day one. Wave 1 has exactly
  one: the current branch. The timeline renderer draws "lines of stations",
  even though wave 1 draws only one.
- Wave 1 has **no Load**, so "Load" keeps a single meaning (time travel with
  auto-branching) when it arrives in wave 2. "Bring back" (§6.6) is a
  different, permanent action: it copies the past into the present.
- Wave 1 never creates branches, never rewrites history (no amend, no reset
  of saves) and never runs merge or rebase.

---

## 5. The main window

Approved direction (mockup shown and accepted in conversation):

```
┌──────────────────────────────────────────────────────────────────────┐
│ ◆ SAVESTATES   [my-portfolio] [recipe-app]  ＋ Open project     ⚙    │
├──────────────────────────────────────────────────────────────────────┤
│ TIMELINE                                                5 saves      │
│   ○────────○────────○────────○────────◉- - - - -(3)                  │
│ First    Contact   Dark    Photo    Phone menu    Now                │
│ save     page      mode    gallery  fix ★         3 unsaved changes  │
│ Oct 12   Oct 14    Oct 20  Oct 26   Today 10:42                      │
├───────────────────────┬──────────────────────────────────────────────┤
│ CHANGED SINCE LAST    │ styles.css             Last save ⟷ Now       │
│ SAVE                  │ ┌ Before ─────────────┐┌ After ─────────────┐│
│  index.html  changed  │ │ .menu {             ││ .menu {            ││
│ ▸styles.css  changed  │ │   gap: 24px;   (red)││   gap: 12px; (green││
│  about.md    new      │ │                     ││   flex-wrap: wrap; ││
│                       │ └─────────────────────┘└────────────────────┘│
│ [ Quick Save  Ctrl+S ]│                                              │
│ [ Throw away changes ]│                                              │
└───────────────────────┴──────────────────────────────────────────────┘
```

- Visual tone: dark, lightly "sci-fi" (glow on the line and on "You are
  here", cyan/indigo accent, amber for unsaved changes, green/red for
  added/removed lines) [call, matching the approved mockup]. Exact theme
  values are [builder]. Raven's theme system and theme editor should hold
  them, not hard-coded colors.
- Layout: project tabs → timeline band (resizable height) → changes list
  on the left, comparison view on the right [decided via mockup].

---

## 6. Wave 1 features

### 6.1 Projects
- **Open project** opens a folder picker. If the folder is a git repository
  (or inside one: use its top level), it opens as a tab. If not, the app
  offers **Start saving this folder** (§6.7.1) [call].
- Several projects can be open at once, one per tab [decided]. Tabs can
  be closed and reordered (reorder is [builder]).
- Open tabs, the active tab and the window size are remembered across
  restarts [call]. A "recent projects" list sits on the empty state / start
  screen [call].
- A project whose folder disappeared shows a plain message in its tab and
  offers to remove it from the list [call].
- **Live refresh**: AI tools edit files while the app is open, so "Now"
  updates by itself. Use a file watcher on the working tree, debounced
  (~300 ms), ignoring `.git/` internals except HEAD/refs changes, plus a
  refresh when the window regains focus [call; mechanism: builder].

### 6.2 Timeline view (metro line)
- **Style: metro line** [decided]. Each save state is a station: a dot on a
  horizontal line, **evenly spaced** (not proportional to time), oldest on
  the left, with its name and date underneath.
- **Pan and zoom** [decided]: drag with the mouse to pan. The wheel scrolls
  horizontally. Ctrl+wheel and touchpad pinch zoom around the cursor. A
  "jump to Now" control recenters [call for the bindings].
- **Level of detail on zoom** [call]: far out shows dots only, the middle
  shows dots and names, close in shows names, date/time and the number of
  files changed. Long names are truncated with an ellipsis. Hovering shows
  the full name, date/time and file count.
- **You are here**: the latest save on the current timeline (git HEAD),
  drawn larger, with a glow and a label [decided via mockup].
- **Now station**: a dashed amber station after "You are here", showing the
  number of unsaved changes. When nothing changed, it is dimmed and labeled
  "Nothing new since your last save" [call].
- On open, the view is scrolled so that "Now" is visible at the right [call].
- **Clicking a station peeks at it** (§6.5). Clicking "Now" goes back to
  the unsaved-changes view. The selected station is highlighted.
- Dates are relative and localized: "today 10:42", "yesterday",
  "Oct 26" / "hoje 10:42", "ontem", "26 de out." [call].
- Scale: history loads in pages (say, 500 saves at a time) as the user pans
  toward the past. Drawing only touches visible stations [builder for the
  numbers].
- Projects already containing branches or merges (Claude Code or a
  developer friend may have made them): wave 1 draws **only the current
  timeline**, meaning the first-parent chain of the current branch. A merge
  save is drawn as an ordinary station (an optional small icon is
  [builder]). A small note says "This project has other timelines. You'll
  be able to see them in a future version" [call].
- Implementation: a Raven **custom widget** (`R_WIDGET_CUSTOM` with
  `Rcustom_set_paint_callback`, drawing with `Rdraw_line` / `Rdraw_circle` /
  `Rdraw_text` / `Rdraw_curve`, input through
  `Rwidget_set_event_callback`) that keeps its own pan/zoom transform
  [call]. Do not use `R_WIDGET_TIMELINE`; it is a media-editing timeline
  (tracks/clips/playhead), not a graph.

### 6.3 Changes panel
- When "Now" is selected: lists every file that differs from the last save,
  including new (untracked, not ignored) files, deleted files and renamed
  files. Each file has a status badge: changed / new / deleted / renamed
  (PT-BR: alterado / novo / apagado / renomeado) [call].
- When a past station is selected: lists the files that save changed
  compared with the save before it (its first parent) [call].
- Selecting a file shows it in the comparison view. The first file is
  selected by default.
- There are no checkboxes and no per-file staging. Quick Save always takes
  everything [decided: "add all"].

### 6.4 Comparison view (diff)
- **Side by side**: Before on the left, After on the right. Removed lines
  are red, added lines green, and changed lines show both with word-level
  highlighting. Unchanged regions far from changes collapse into
  "… 42 unchanged lines …", which expands on click [call]. There is no
  unified-diff mode in wave 1 [call].
- Headers name both sides in user terms: "Before · <save name>",
  "After · Now" or "After · <save name>".
- New file: Before is empty with "This file didn't exist yet". Deleted file:
  After is empty with "This file was deleted" [call].
- Binary files: "This file can't be shown as text (image or other file
  type)". Showing before/after for common image formats is a nice-to-have
  [builder].
- Huge diffs (say, > 5,000 lines or > 1 MB) show a summary and a "Show
  anyway" button [call; thresholds: builder].
- Text is decoded as UTF-8 (lossy for anything else). Read-only.
  Line-ending-only changes are labeled "Only line endings changed" instead
  of showing every line red/green [call].
- Implementation: get both file versions from git (`git show <rev>:<path>`
  / working-tree file), compute the alignment in Rust (for example with the
  `similar` crate), and render either with read-only Raven `Text Editor`
  widgets plus markers or with a custom-painted widget [builder: pick
  whichever gives full-line backgrounds and synchronized scrolling].

### 6.5 Peek (viewing a past save)
- Clicking a station opens peek mode in the lower panels. A banner reads
  "Peeking at <name> · <date>" with a "Back to Now" button [call].
- Peek mode has **two tabs** [call]. A save state holds the whole project,
  not just what changed, so the user can browse all of it:
  - **What changed** (default): the files this save changed compared with
    the save before it. The comparison view shows Before (the previous
    save) and After (this save).
  - **All files**: a read-only folder tree of the entire project as it was
    at this save, with a filter box. Selecting a file compares
    "<this save> ⟷ Now", showing what bringing it back would change. Files
    that no longer exist show "This file doesn't exist anymore" on the Now
    side.
- The user's files on disk are **not touched** by peeking.
- The banner also shows the full save name. Authors are not shown in
  wave 1 (single user) [call].
- Typical "the AI broke x.js today" story: peek at yesterday's station,
  open **All files**, select `x.js`, see the comparison with Now, then use
  **Bring back this file** (§6.6).

### 6.6 Bring back [call]
The wave-1 way to get the past back without time travel:
- **Bring back this file**, in the peek view, per file: writes that file's
  content into the working folder. It shows up as an unsaved change in
  "Now". If that file has unsaved changes, confirm first and keep a
  recovery snapshot (§6.8.2). Which version comes back:
  - In the **All files** tab: the file as it was at the peeked save.
  - In the **What changed** tab there are two buttons under the comparison:
    "Bring back the After version" (as it was at this save) and "Bring back
    the Before version" (as it was just before this save, which is what
    undoes a save that broke the file). For a file this save created,
    "Before" means deleting the file and is labeled that way. For a file
    this save deleted, only "Before" applies [call].
- **Bring back everything from this save**, in the peek banner: makes the
  whole working folder match that save (restoring, adding and removing
  tracked files; ignored files untouched). The result shows up as unsaved
  changes. A Quick Save then records it as a new save at the end of the
  line. History stays linear and nothing newer is lost.
  - If there are unsaved changes, first offer "Quick Save first",
    "Bring back anyway (your unsaved changes go to the recovery area)", or
    Cancel.
  - Underlying operation (suggested):
    `git restore --source=<sha> --worktree -- :/`. It runs in no-overlay
    mode, so tracked files absent from `<sha>` are removed. Leave the index
    alone [builder to verify edge cases].
- Peek, then Bring back, then Quick Save is the whole wave-1 "go back" story.

### 6.7 Quick Save [decided]
- A primary button in the changes panel, plus **Ctrl+S**.
- It is disabled when there are no changes, with a tooltip saying "Nothing
  new since your last save".
- Flow:
  1. **Safety check** (§6.8.1): if anything looks like junk, a secret or a
     very large file, show the warning dialog first.
  2. **Name**, according to the setting:
     - *Ask me for a name* (default) [decided]: a small dialog with one
       text field, prefilled empty, with a placeholder such as "What did
       you change?". Enter saves, Escape cancels. An empty name is not
       allowed [call].
     - *Let AI name it* [decided]: run Claude (§6.9) and show the same
       dialog **prefilled with the suggestion**, which stays editable. A
       spinner covers the wait [call: the user can always edit or replace
       the AI name]. If Claude is unavailable, fails or times out, show
       the empty dialog with a one-line note ("Couldn't get a name from the
       AI: <reason>") [call].
  3. **Save**: `git add -A` then `git commit -m <name>` (with
     `--cleanup=verbatim` or an equivalent, so names starting with `#`
     survive) [call].
  4. The new station animates into the line (Raven's spring animations),
     "You are here" moves onto it, and "Now" empties [call].
- **Project hooks**: if the repository has commit hooks (husky etc.) they
  run normally. Hooks are never skipped. When a hook rejects the save,
  say "This project's automatic checks stopped the save", with the hook
  output in a details expander [call].
- An empty repository (no saves yet) works: the first Quick Save creates
  the first station [call].

#### 6.7.1 Start saving this folder (git init) [call]
- `git init` (default branch name `main`). If the folder has no
  `.gitignore`, write one from a built-in template covering OS junk
  (`.DS_Store`, `Thumbs.db`, `desktop.ini`), dependency/build folders
  (`node_modules/`, `dist/`, `build/`, `out/`, `.next/`, `target/`,
  `__pycache__/`, `.venv/`, `venv/`), secrets (`.env`, `.env.*` with
  `!.env.example`), and editor/log noise (`*.log`, `.idea/`, `.vs/`).
  An existing `.gitignore` is never modified without asking.
- It then offers a first Quick Save named "Começo do projeto" / "Project
  start", which the user can edit.

### 6.8 Safety rails

#### 6.8.1 Junk, secrets and large files [call]
Before every Quick Save, check the files about to be saved:
- **Junk folders**: known dependency/build folders (as in §6.7.1) that are
  not ignored. Offer "Leave these out (recommended)", which appends them to
  `.gitignore` and shows that as part of the save.
- **Possible secrets**:
  - By name: `.env`, `.env.*` (not `.env.example`), `*.pem`, `*.key`,
    `*.p12`, `*.pfx`, `id_rsa*`, `id_ed25519*`, `credentials*.json`,
    `*.keystore`.
  - By content, on new or changed text files, matching common token
    patterns: private-key headers, `AKIA…`, `ghp_…`/`github_pat_…`,
    `sk-ant-…`, `sk-…`, `xox[bp]-…`, `AIza…`.
- **Large files**: anything over 25 MB [builder may tune].
- The warning dialog lists the flagged files with plain reasons, plus
  per-file actions. Quick Save stays "everything, every time": the app
  never makes a partial save. The available actions depend on the case
  [call]:

  | Case | Actions (default first) |
  |---|---|
  | New, not-yet-saved file flagged by **name** (e.g. `.env`) or a junk folder | **Leave out** (add to `.gitignore`; the `.gitignore` change is part of this save) · Save anyway |
  | **Already-saved** file flagged by name (e.g. a `.env` committed long ago) | **Stop saving this file** (`git rm --cached` + add to `.gitignore`; the file stays on disk, and the save records it as no longer saved) · Save anyway |
  | Any file flagged by **content** (a key pasted into `src/api.js`) | **Cancel and fix it** (closes the dialog and selects the file so the user can see the flagged line) · Save anyway. "Leave out" is not offered, because ignoring a source file would be wrong. |
  | Large file | **Leave out** (ignore by exact path) · Save anyway. For an already-saved large file: Stop saving this file · Save anyway. |

- When a flagged file **already exists in older saves**, the dialog says
  so plainly: "Older saves still contain this file. That's fine on this
  computer, but keep it in mind before sharing the project." History is
  never rewritten [call]. This matters more once a later wave adds push.

#### 6.8.2 Throw away changes, recoverably [call]
- "Throw away changes" (all, or one file from the changes-list context
  menu) asks for confirmation, then **snapshots the discarded state
  before discarding**, so nothing is unrecoverable:
  - Suggested mechanism: record the working tree, including untracked,
    non-ignored files, as a commit object, for example
    `git stash push --include-untracked` immediately followed by moving the
    entry to a private ref such as `refs/savestates/recovery/<timestamp>`
    and dropping it from the stash list. The point is to stay out of the
    user's visible stash and out of other tools' way [builder].
- Afterwards, show a toast with **Undo** for ~10 seconds. A small
  "Recovery" list in the project menu lets the user restore anything
  discarded in the last 30 days. Older entries are pruned [call].
- "Bring back anyway" (§6.6) uses the same recovery area.

#### 6.8.3 Who is saving (git identity) [call]
- If git has no `user.name` / `user.email` for the project, the first Quick
  Save asks once: "What name should go on your saves?" plus an email (with
  a note that any email works and it's stored with their saves). The values
  are written to the global git config. Existing config is never
  overwritten.

#### 6.8.4 Repositories in states the app doesn't handle [call]
Detect them and switch the tab to a **read-only** mode with a plain
explanation and no actions that could make things worse:
- a merge, rebase, cherry-pick, revert or bisect in progress (any
  `.git/MERGE_HEAD`, `rebase-merge/`, `rebase-apply/`, `CHERRY_PICK_HEAD`,
  `REVERT_HEAD`, `BISECT_LOG`);
- detached HEAD ("This project is looking at an old save from another
  program");
- unmerged/conflicted index entries;
- the folder is a bare repository, or git reports corruption.

The timeline and diffs stay viewable. Quick Save, Bring back and Throw
away are disabled with that explanation. A "Check again" button
re-detects. Submodules and LFS files appear as ordinary entries, and
special handling is out of scope.

#### 6.8.5 Git missing [call]
If `git` isn't found (on PATH or in the standard Git for Windows
locations), show a start screen explaining that SaveStates needs Git, with
a button that opens the Git for Windows download page. Minimum git version
2.30 [builder may adjust], checked at startup.

### 6.9 AI naming with Claude [decided: `claude -p` with Haiku]
- Runs the Claude Code CLI headlessly. Suggested invocation [call]:
  ```
  claude -p "<instruction>" --model <model> --tools "" \
         --no-session-persistence --output-format json
  ```
  The diff goes in through **stdin**.
  - `<model>` comes from settings, defaulting to `haiku` (the alias for the
    current Haiku, `claude-haiku-5-5` today) [call].
  - Run it from a **neutral working directory** (the app's temp/cache
    folder), not the project folder, so the project's `CLAUDE.md`, hooks
    and settings don't load. The repository content is untrusted input to
    the prompt [call].
  - Do not use `--bare`; it skips keychain reads, which can break the
    user's normal Claude login [call].
  - `--json-schema` can be used to get a strict `{"name": "..."}` result
    [builder].
- **What gets sent**: the result of "add all" without touching the real
  index. Build it with a temporary index (copy `.git/index` to a temp file,
  run `GIT_INDEX_FILE=<tmp> git add -A`, then
  `GIT_INDEX_FILE=<tmp> git diff --cached --stat` and `--patch`) [call:
  the user's real index must not change if they cancel]. Exclude files the
  safety check marked "leave out", and never include file contents flagged
  as secrets. Truncate the patch to a size cap (say, 20 KB, keeping the
  stat summary whole) [builder for the cap].
- **Instruction** [call]: write one short save name (≤ 72 characters,
  no trailing period, no quotes, no prefixes like "feat:") describing what
  changed, in plain words a non-programmer would understand, **in the UI
  language** (PT-BR or English). Return only the name.
- **Timeouts and failures**: a 30-second timeout (configurable), and the
  call can be cancelled from the spinner. Not installed, not logged in,
  non-zero exit, empty output or timeout all fall back to the manual
  dialog with a short reason (§6.7) [call].
- **Privacy notice**: the first time AI naming is switched on, show a
  one-time notice that the changes in each save are sent to Anthropic
  through the user's Claude Code account [call].
- Detecting Claude: look for `claude` on PATH (and the standard install
  locations) when the setting is turned on. If it's not found, explain
  and keep "Ask me" selected [call].

### 6.10 Settings [call unless noted]
Stored per user in the OS config folder (`%APPDATA%\SaveStates\settings.toml`
on Windows, through the `directories` crate or similar). Nothing app-specific
is written inside projects, apart from the `.gitignore` it creates and the
private recovery refs.

| Setting | Values | Default |
|---|---|---|
| Save naming [decided] | Ask me for a name / Let AI name it | Ask me |
| AI model (advanced) | free text | `haiku` |
| AI timeout (advanced) | seconds | 30 |
| Language | System / Português (Brasil) / English | System (§7.6) |
| Large-file warning | MB | 25 |

Also stored: open tabs, recent projects, window geometry, and timeline
zoom per project.

---

## 7. Technical decisions

### 7.1 Language and UI framework [decided]
- **Rust** desktop app.
- **Raven by Oscar Soirey** (https://github.com/oscar-soirey/Raven), a C11
  GUI library with Rust bindings (`bindings/rust`, used as `raven`).
  Assessed as adequate. It has a custom-painted widget with input events
  (timeline), `Rtask_run_async` / `Rtask_post` for background work,
  read-only text editors with markers, virtualized list views, native file
  dialogs, built-in translation tables, screen-reader support and
  spring animations.

### 7.2 Raven risks and how to contain them [call]
| Risk | Mitigation |
|---|---|
| Very new (first release 2026-10-10, now v0.2.0). The README warns the public API will change. | Pin an exact release/commit. All Raven calls go through one internal crate (`ui-raven` or similar). Nothing else touches `raven::sys`. |
| The Rust binding is raw `unsafe` FFI (generated `sys.rs`) with only a few helpers. | Write a small safe wrapper for exactly the calls we use: `Rid` newtypes, `&str` → C string, closures for callbacks, and UI-thread-only checks via `Ris_ui_thread`. |
| `raven-gui` on crates.io belongs to an unrelated project (a Uxn emulator GUI), so the crate can't come from crates.io. | Use a git dependency pinned to a tag (or vendor `bindings/rust`). Link the prebuilt Raven library from the matching GitHub release through `RAVEN_LIB_DIR`. Ship `raven.dll` beside the exe, or link statically with `RAVEN_STATIC=1`. |
| **Vulkan only**, no software renderer. On personal Windows PCs the realistic failures are outdated or generic graphics drivers and very old hardware. Virtual machines and remote desktop are not target scenarios. | Detect failure at startup and show a native message box explaining the problem (with "update your graphics driver" guidance) instead of crashing. Bundling a software Vulkan driver (Mesa lavapipe) is optional and decided by the §9 spike [builder]. |
| Single maintainer, young project. | The wrapper crate keeps a future switch to another toolkit contained. |

- UI layout lives in Raven **XML files** (`OnClicked="…"` actions, widgets
  found by `Name`), embedded into the binary with `include_str!` for
  release builds. In debug builds they load from disk for Raven's live
  reload [call].
- Threading rule: the UI thread never runs git or Claude. Work goes to
  `Rtask_run_async` (or a Rust thread pool), and results come back with
  `Rtask_post` [call].

### 7.3 Git access [call]
- Run the **system `git` executable** (it is a hard requirement of Claude
  Code on Windows, so the audience already has it). Do not use libgit2 or
  gitoxide for writes.
  - Reasons: identical behavior to the git the user's other tools run;
    hooks, `.gitattributes`, credential helpers and future rebase/push work
    out of the box; less risk of corrupting repos.
  - Reading with `gix` for speed (history pages, blobs) is allowed if
    profiling shows the CLI is too slow [builder].
- Always use machine-readable output: `git status --porcelain=v2 -z
  --untracked-files=all`, `git log -z --format=…` with explicit fields,
  `git diff --name-status -z`, `git show <rev>:<path>`.
- The process environment sets `GIT_TERMINAL_PROMPT=0`,
  `GIT_OPTIONAL_LOCKS=0` for read commands, `LC_ALL=C` /
  `LANGUAGE=en` so parsing doesn't depend on the locale, and on Windows
  `CREATE_NO_WINDOW` so console windows never flash.
- Commands for one project run one at a time through a per-project queue,
  so a refresh never races a save. Reads may be cancelled and superseded.

### 7.4 Concept → git mapping (wave 1)
| User action | Git underneath |
|---|---|
| Open project | `git rev-parse --show-toplevel`, state detection (§6.8.4) |
| Start saving this folder | `git init -b main`, write `.gitignore` |
| See unsaved changes | `git status --porcelain=v2 -z --untracked-files=all` |
| Timeline | `git log --first-parent -z --format=<fields> <branch>` (paged) |
| Peek at a save | `git diff --name-status -z <sha>^1 <sha>` (root commit: against the empty tree) |
| Compare a file | `git show <rev>:<path>` for each side, diffed in Rust |
| Quick Save | `git add -A` + `git commit --cleanup=verbatim -m <name>` |
| AI name input | temporary-index `add -A` + `diff --cached` (§6.9) |
| Peek "All files" tree | `git ls-tree -r -z --name-only <sha>` |
| Bring back a file (at this save / After) | `git restore --source=<sha> --worktree -- <path>` |
| Bring back a file (Before this save) | `git restore --source=<sha>^1 --worktree -- <path>` (removes it if absent there) |
| Stop saving this file | `git rm --cached -- <path>` + append to `.gitignore`, then the normal save |
| Bring back everything | `git restore --source=<sha> --worktree -- :/` |
| Throw away changes | snapshot to `refs/savestates/recovery/*`, then restore + clean of non-ignored files |
| Identity | `git config user.name/user.email` (global write only if missing) |

### 7.5 Suggested code structure [call; exact names are builder]
```
savestates/            (cargo workspace)
  app/        binary: startup, windows, wiring XML actions to the core
  ui-raven/   the only crate that touches raven::sys; safe wrapper
  core/       domain: Project, Timeline, SaveState, Change, FileDiff,
              settings, safety checks. No UI and no process spawning.
  git/        runs git, parses porcelain output, repo-state detection
  ai-namer/   runs `claude -p`, builds the prompt, timeouts
  i18n/       pt-BR.csv + en.csv (or one CSV with both columns), key lists
```
- `core` and `git` are tested against real temporary repositories
  (integration tests that create repos, files and saves, then assert on
  results), including every §6.8.4 state [call].
- UI smoke tests can drive the window through Raven's
  `Rwindow_inject_*` functions [builder].

### 7.6 Translation [decided: PT-BR + EN; call: mechanism]
- Use Raven's built-in system: CSV tables (`Rlocale_load_csv_from_memory`,
  embedded), `Text="@key"` in XML, `Rtr` / `Rtr_format` / `Rtr_plural` in
  code, and `Rlocale_format_number` for numbers.
- **PT-BR is the primary language**: written first, complete, and the
  fallback (`Rlocale_set_fallback_language("pt-BR")`). English is complete
  too.
- Language choice: the setting, or by default the system language
  (`Rlocale_get_system_language`). Any `pt*` → PT-BR, any `en*` → English,
  anything else → PT-BR.
- Dates use localized formats. Raven has no date formatter, so use
  `chrono` with our own PT-BR/EN month names, or an ICU-based crate
  [builder].
- No user-facing string is hard-coded. A CI check fails on a key missing
  from either language [call].
- Check the layouts in PT-BR (longer strings) as the main case.
- Git output is never shown untranslated, except inside "details"
  expanders.

### 7.7 Packaging [call]
- Windows x64 installer (MSI or Inno Setup, per user, no admin rights
  needed), including `raven.dll` (unless statically linked) and an app
  icon. **Code signing is deferred** [decided: personal computers only].
  Windows SmartScreen will warn on the unsigned installer, so the download
  page or README tells friends how to get past it ("More info" → "Run
  anyway"), in PT-BR.
- Git and Claude Code are prerequisites, detected and not bundled.

---

## 8. Wave 2 and 3 direction (not built in wave 1)

Recorded so wave 1 doesn't paint itself into a corner.

- **Load (wave 2)** [decided in spirit]: clicking a station and choosing
  Load moves the user to that point (git: detached HEAD or a lazily created
  branch). The working folder now looks like that save. A Quick Save from
  there **automatically creates a new timeline** (git branch, auto-named,
  shown as a new parallel metro line). Unsaved changes at the moment of
  Load need a policy (save first / carry along / park in recovery), to be
  designed in wave 2.
- **Timelines** are drawn as parallel lines branching off a station,
  metro-map style.
- **Joining timelines (wave 3)** [decided]: rebase the source timeline onto
  the target, then fast-forward, so history stays a single line. Conflicts
  get a guided, plain-language resolution UI.
- **Remotes (later)**: pull with rebase, push, and a plain sync indicator.

---

## 9. Risks and suggested early spikes
1. **Raven + Vulkan on the friends' personal PCs** [medium]. Before
   building features, spike a "hello window" (with a custom-painted widget
   and text) on (a) a typical laptop with integrated Intel/AMD graphics,
   (b) a desktop with a discrete GPU, and if possible (c) an older machine
   from one of the friends. The result decides whether a lavapipe fallback
   is worth shipping in wave 1. **This is a go/no-go checkpoint**: if Raven can't run
   acceptably on the target machines (§1, "Who it's for"), even with a
   fallback, stop and report to the product owner before building
   features. Switching toolkits is the owner's decision. Raven is not to
   be replaced unilaterally [decided: "if you find that inadequate, let me
   know"].
2. **Raven custom widget for pan/zoom** [medium]. Spike the metro line with
   10,000 stations: drawing performance, input events, crisp text at zoom.
3. **Diff rendering in Raven** [medium]. Confirm the text editor's markers
   can do full-line backgrounds and synchronized scrolling. Otherwise use a
   custom-painted diff widget.
4. **Raven API churn** [ongoing]. Contained by pinning and the wrapper crate.
5. **`claude -p` behavior changes** [low]. Contained by the fallback to
   manual naming.

---

## 10. Left to the builder
- Exact theme values, icons, animations and spacing (within §5's tone).
- Diff algorithm and rendering approach (§6.4).
- Thresholds: history page size, diff size cap, AI diff cap, large-file
  size, refresh debounce.
- The file-watcher crate and the strategy for ignoring `.git` noise.
- Exact recovery-snapshot mechanism (§6.8.2), as long as it is invisible to
  other tools, recoverable, and pruned after 30 days.
- Installer technology and update mechanism.
- Whether to read history with `gix` for speed.
- Keyboard shortcuts beyond Ctrl+S.
