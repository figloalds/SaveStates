//! The start screen (`start.xml`): shown when no project is open, with the recent projects, or
//! in place of everything else when Git is missing (spec §6.1 and §6.8.5).

use std::path::{Path, PathBuf};

use savestates_core::settings::MAX_RECENT;
use savestates_ui_raven::{Widget, widget_from_xml};

use crate::ui_files;

/// Where the "download Git" button leads.
pub const GIT_DOWNLOAD_URL: &str = "https://git-scm.com/download/win";

/// One line of the recent projects: the folder's name as a button, then its full path.
struct RecentRow {
    row: Widget,
    open: Widget,
    path: Widget,
}

pub struct StartScreen {
    root: Widget,
    normal: Widget,
    git_missing: Widget,
    recent: Widget,
    rows: Vec<RecentRow>,
    /// The folders the rows show, in order.
    shown: Vec<PathBuf>,
}

impl StartScreen {
    /// Builds the screen from `start.xml`. It is not in a window yet: see [`StartScreen::widget`].
    pub fn new() -> Result<StartScreen, String> {
        let root = widget_from_xml(&ui_files::source("start.xml"))?;
        let find = |name: &str| {
            root.find(name)
                .ok_or_else(|| format!("start.xml has no widget named {name}"))
        };
        let mut rows = Vec::new();
        for i in 0..MAX_RECENT {
            rows.push(RecentRow {
                row: find(&format!("recent{i}"))?,
                open: find(&format!("recent{i}.open"))?,
                path: find(&format!("recent{i}.path"))?,
            });
        }
        Ok(StartScreen {
            normal: find("start.normal")?,
            git_missing: find("start.git_missing")?,
            recent: find("start.recent")?,
            root,
            rows,
            shown: Vec::new(),
        })
    }

    pub fn widget(&self) -> Widget {
        self.root
    }

    /// Shows only the explanation that Git is needed, with its download button.
    pub fn show_git_missing(&self) {
        self.normal.set_visible(false);
        self.git_missing.set_visible(true);
    }

    /// Lists `recent` (newest first, at most [`MAX_RECENT`]); the section hides when it is empty.
    pub fn set_recent(&mut self, recent: &[PathBuf]) {
        self.shown = recent.iter().take(self.rows.len()).cloned().collect();
        for (i, row) in self.rows.iter().enumerate() {
            match self.shown.get(i) {
                Some(path) => {
                    row.open.set_button_text(&folder_name(path));
                    row.open.set_tooltip(Some(&path.to_string_lossy()));
                    row.path.set_label_text(&path.to_string_lossy());
                    row.row.set_visible(true);
                }
                None => row.row.set_visible(false),
            }
        }
        self.recent.set_visible(!self.shown.is_empty());
    }

    /// The recent project whose button is `sender`.
    pub fn recent_for(&self, sender: Widget) -> Option<PathBuf> {
        let i = self.rows.iter().position(|row| row.open == sender)?;
        self.shown.get(i).cloned()
    }
}

/// The name a folder is shown by: its last component, or the whole path for a drive root.
pub fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_are_shown_by_their_last_name() {
        assert_eq!(
            folder_name(Path::new(r"C:\Users\ana\my-portfolio")),
            "my-portfolio"
        );
        assert_eq!(folder_name(Path::new(r"C:\")), r"C:\");
    }
}
