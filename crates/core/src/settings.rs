//! Per-user preferences and session state, stored as a TOML file.
//!
//! A missing or damaged file never stops the app: `load` falls back to the defaults, and a
//! damaged file is kept aside as `<name>.bad` so nothing a person wrote is lost.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use savestates_i18n::LanguageChoice;
use serde::{Deserialize, Serialize};

/// How many recent projects are remembered.
pub const MAX_RECENT: usize = 10;

/// How a save gets its name.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NamingMode {
    /// Ask the person to type a name.
    #[default]
    AskMe,
    /// Let the AI suggest a name.
    Ai,
}

/// Where the main window was, in screen pixels.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct WindowGeom {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
}

/// Everything the app remembers between runs. Fields missing from the file take their defaults
/// and fields this version does not know are ignored.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub naming: NamingMode,
    pub ai_model: String,
    pub ai_timeout_secs: u64,
    pub language: LanguageChoice,
    /// Files above this size (in MB) are flagged before a save.
    pub large_file_mb: u64,
    /// Whether the notice that the AI sees the changes was already shown.
    pub ai_notice_shown: bool,
    pub open_tabs: Vec<PathBuf>,
    pub active_tab: usize,
    /// Recently opened projects, newest first.
    pub recent: Vec<PathBuf>,
    pub window: Option<WindowGeom>,
    /// The timeline's zoom per project, keyed by the project's path: how wide a day is, in
    /// pixels. It replaces wave 1's `timeline_zoom`, whose values meant something else: that key
    /// is ignored on load and gone from the file at the next save.
    pub timeline_day_px: BTreeMap<String, f32>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            naming: NamingMode::AskMe,
            ai_model: "haiku".to_owned(),
            ai_timeout_secs: 30,
            language: LanguageChoice::System,
            large_file_mb: 25,
            ai_notice_shown: false,
            open_tabs: Vec::new(),
            active_tab: 0,
            recent: Vec::new(),
            window: None,
            timeline_day_px: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// Puts `path` first in the recent projects. Another entry for the same folder (however it
    /// is spelled) is dropped, and only the newest `MAX_RECENT` are kept.
    pub fn push_recent(&mut self, path: PathBuf) {
        let key = identity(&path);
        self.recent.retain(|other| identity(other) != key);
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT);
    }
}

/// The path with `.`, `..`, links and letter case resolved, or as given when it does not exist.
fn identity(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Reads the settings. Never fails and never panics:
/// - a missing file gives the defaults;
/// - an unreadable or unparseable file gives the defaults and is renamed to `<name>.bad`
///   (replacing an older one), so the next `save` does not overwrite what was there.
pub fn load(path: &Path) -> Settings {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Settings::default(),
        Err(_) => {
            set_aside(path);
            return Settings::default();
        }
    };
    match toml::from_str(&text) {
        Ok(settings) => settings,
        Err(_) => {
            set_aside(path);
            Settings::default()
        }
    }
}

/// Moves a damaged file to `<name>.bad`. If even that fails the defaults are still used and the
/// next `save` replaces the file, so there is nothing more that can be done here.
fn set_aside(path: &Path) {
    let _ = fs::rename(path, sibling(path, ".bad"));
}

/// `path` with `suffix` appended to its file name, in the same folder.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Writes the settings, creating the folder if needed. The data goes to `<name>.tmp` first and
/// is then renamed over the real file, so a crash never leaves half a file behind.
pub fn save(path: &Path, s: &Settings) -> io::Result<()> {
    let text = toml::to_string_pretty(s)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    let temp = sibling(path, ".tmp");
    let written = write_synced(&temp, text.as_bytes()).and_then(|()| fs::rename(&temp, path));
    if written.is_err() {
        // Leave no stray temp file behind; the error that matters is the one being returned.
        let _ = fs::remove_file(&temp);
    }
    written
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// `%APPDATA%\SaveStates\settings.toml`.
pub fn default_path() -> PathBuf {
    let folder = match ProjectDirs::from("", "", "SaveStates") {
        Some(dirs) => app_folder(dirs.config_dir()),
        None => std::env::temp_dir().join("SaveStates"),
    };
    folder.join("settings.toml")
}

/// On Windows `directories` puts the config folder one level down (`...\SaveStates\config`),
/// but the settings file lives right in `%APPDATA%\SaveStates`.
fn app_folder(config_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        config_dir.parent().unwrap_or(config_dir).to_path_buf()
    } else {
        config_dir.to_path_buf()
    }
}
