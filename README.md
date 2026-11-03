# SaveStates

A friendly desktop git client for non-technical people (Windows x64 only). It lets someone keep versions of a project ("save states"), look back at any of them and bring files back, and load any save to carry on from there (a new direction becomes its own timeline, drawn as a colored lane, and Undo goes back), without learning git. The UI is Brazilian Portuguese first, with English as the second language. It is built in Rust on the [Raven](https://github.com/oscar-soirey/Raven) GUI library (Vulkan) and runs the system `git` executable.

Friends get an unsigned per-user installer and a PT-BR user guide: see [LEIA-ME.md](LEIA-ME.md).

## Where things are

- Design spec: [`docs/superpowers/specs/2026-11-01-savestates-design.md`](docs/superpowers/specs/2026-11-01-savestates-design.md)
- Wave 1 implementation plan: [`docs/superpowers/plans/2026-11-01-savestates-wave1.md`](docs/superpowers/plans/2026-11-01-savestates-wave1.md)
- Raven spike go / no-go checklist: [`docs/spike-checklist.md`](docs/spike-checklist.md)

## Crates

| Crate (package) | What it is |
|---|---|
| `crates/app` (`savestates`) | The app: windows, timeline, dialogs, flows. Builds `savestates.exe`. |
| `crates/core` (`savestates-core`) | Domain model and operations: projects, timeline, diff, safety scan, Quick Save, recovery, bring back, settings. |
| `crates/git` (`savestates-git`) | Thin, safe wrapper around the system `git` (2.31 or newer). |
| `crates/ai-namer` (`savestates-ai`) | Names saves with `claude -p`. |
| `crates/i18n` (`savestates-i18n`) | PT-BR / English table (`translations.csv`), language resolution, relative dates. |
| `crates/ui-raven` (`savestates-ui-raven`) | Safe wrapper over Raven. The only crate with `unsafe` and the only one that depends on Raven. |
| `crates/testkit` | Test helpers (temporary git repos). Dev-dependency only, never shipped. |
| `vendor/raven-gui` | Raven's Rust binding, vendored verbatim from the release zip (excluded from the workspace). |
| `third_party/raven-bin` | Raven's static library and notices, fetched by script (git-ignored). |

Every user-facing string lives in `crates/i18n/translations.csv` (header `key,pt-BR,en`); a test checks that the code and the table agree.

## Prerequisites

- Windows 10 or 11, x64.
- Rust through `rustup`; `rust-toolchain.toml` pins 1.96.0. The MSVC build tools with a Windows SDK are needed too (the SDK's `rc.exe` embeds the icon).
- Git for Windows 2.31 or newer on `PATH`.
- Python 3.11 or newer (for `scripts/build.py`).
- PowerShell 7 (`pwsh`) for the other scripts.
- Only to build the installer: Inno Setup 6.3 or newer (`winget install JRSoftware.InnoSetup`).

## Getting started

Fetch Raven v0.2.0 (static MSVC x64, SHA-256 checked) once:

```powershell
pwsh scripts/fetch-raven.ps1
```

Then build and test:

```powershell
cargo build -p savestates
cargo run -p savestates
cargo test --workspace
cargo clippy -p savestates --all-targets
cargo fmt -p savestates
```

Format only your own crates, never `vendor/`.

To try the timeline by hand, create a sample project in `%TEMP%\savestates-sample` (1,500 saves with PT-BR names, a merged `feature` timeline and 3 unsaved changes):

```powershell
pwsh scripts/make-sample-repo.ps1
```

## Release build

```powershell
python scripts/build.py
```

This compiles a snapshot of the working tree (uncommitted changes included) in release mode and writes the runnables to `target/runnables/windows-x86_64/`:

- `savestates.exe`, the app, with the icon and version information embedded and no console window;
- `savestates-spike.exe`, the Raven spike;
- `vcruntime140.dll` next to them (the exe is statically linked with Raven, so it is the only extra DLL), plus Raven's `RAVEN-LICENSE.txt` and `RAVEN-NOTICE.txt`;
- `BUILD.json`, with the commit, toolchain and each exe's SHA-256.

Useful flags: `--what app` (only the app), `--smoke` (launch each exe for a few seconds to check it stays up; it opens a window), `--keep-build-cache` (faster rebuilds), `--profile debug`, `--offline`. It fetches Raven by itself if it is missing. `python scripts/build.py --help` lists the rest.

### The installer

```powershell
python scripts/build.py --installer
```

This runs `cargo test --workspace --locked` (skip with `--skip-tests`), builds the app in release mode (the installer only holds the app, so the spike is skipped), and compiles `installer/savestates.iss` with Inno Setup into `dist/SaveStates-Setup-<version>.exe`. The version comes from `[workspace.package]` in the root `Cargo.toml`. `ISCC.exe` is found on `PATH` or in the usual `Inno Setup 6` folders; if it is not there, the script stops and says how to install it.

The installer is per-user (no administrator rights, `%LOCALAPPDATA%\Programs\SaveStates`), has a Start menu shortcut and an uninstaller, and speaks Brazilian Portuguese and English. It is **not code-signed**, so Windows SmartScreen warns about it; `LEIA-ME.md` walks friends through "Mais informações" and "Executar assim mesmo".

To compile `installer/savestates.iss` by hand, run `python scripts/build.py --what app` first, then open the script in the Inno Setup compiler (its defaults point at `target/runnables/windows-x86_64`).

### Logs

A release build is a GUI program with no console, so the app writes its diagnostics and any panic (message, file, line and thread) to `%APPDATA%\SaveStates\log.txt`, next to `settings.toml`. Each line starts with the local time. Once the file passes 2 MB only its last megabyte is kept. A debug build also prints the lines to stderr.

### The icon

`assets/savestates.ico` (256, 48, 32 and 16 px) is drawn by `python scripts/make-icon.py` (plain Python, no dependencies) and embedded by `crates/app/build.rs` through `winresource`.

## Conventions

- Windows x64 is the only target. Any Windows-only call sits behind `#[cfg(windows)]`.
- `unsafe` lives only in `crates/ui-raven` (and the dev-only `testkit`); every other crate forbids it.
- The UI thread never runs git or `claude`; work goes to a worker thread and comes back through `ui_raven::task::post`.
- Tests that use `testkit::TestRepo` live in a crate's `tests/` directory.
