//! A list of changed files (spec §6.3, addendum §5): the unsaved changes while "Now" is
//! selected, and what a peeked save changed. One row per file, with a colored symbol for its
//! change, its name and its folder (see [`crate::change_rows_widget`]); selecting a row shows
//! that file in the comparison view. The unsaved changes also have a context menu (spec §6.8.2:
//! throw away one file).

use savestates_core::Change;
use savestates_ui_raven::{Widget, Window};

use crate::change_rows_widget::{self, RowsWidget};

/// The changes shown in a list that paints only the rows in view.
pub struct ChangesPanel {
    rows: RowsWidget,
}

impl ChangesPanel {
    /// Shows changes in a list put inside `list_host` (an `Overlay`), none yet. `on_select` gets
    /// the change the user selects; a selection made by [`ChangesPanel::set_changes`] or
    /// [`ChangesPanel::select_path`] is not reported.
    pub fn new(list_host: Widget, on_select: impl FnMut(Change) + 'static) -> ChangesPanel {
        let (rows, widget) = change_rows_widget::new(on_select);
        list_host.add_child(widget);
        ChangesPanel { rows }
    }

    /// Shows `changes`. The file that was selected stays selected while it is among them,
    /// otherwise the first one is. Returns the selected change.
    pub fn set_changes(&self, changes: Vec<Change>) -> Option<Change> {
        let was = self.selected().map(|change| change.path);
        let row = pick_row(was.as_deref(), &changes);
        self.rows.set_rows(changes, row);
        self.selected()
    }

    /// Shows the rows again, in the current language, keeping the selection.
    pub fn relabel(&self) {
        self.rows.repaint();
    }

    /// Opens `menu` on a right click on the list, in `window`. A right click on a row selects
    /// it first (and reports it, like a click), so the menu acts on that file.
    pub fn set_context_menu(&self, window: Window, menu: Widget) {
        self.rows.set_context_menu(window, menu);
    }

    /// Selects the change of the file at `path` and returns it; `None` (and nothing changes)
    /// when that file has no change shown. Like any selection made in code, it is not reported.
    pub fn select_path(&self, path: &str) -> Option<Change> {
        let row = self.rows.position(path)?;
        self.rows.select(Some(row));
        self.selected()
    }

    /// The selected change.
    pub fn selected(&self) -> Option<Change> {
        self.rows.selected()
    }

    /// The changes shown.
    pub fn changes(&self) -> Vec<Change> {
        self.rows.changes()
    }
}

/// Which row to select among `changes`: the one of the file at `was` (the path selected before)
/// while it is there, else the first. `None` when there are no changes.
fn pick_row(was: Option<&str>, changes: &[Change]) -> Option<usize> {
    let kept = was.and_then(|path| changes.iter().position(|change| change.path == path));
    kept.or((!changes.is_empty()).then_some(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use savestates_core::ChangeKind;

    fn change(path: &str) -> Change {
        Change {
            path: path.to_owned(),
            kind: ChangeKind::Changed,
        }
    }

    #[test]
    fn the_selected_file_stays_selected_else_the_first_one_is() {
        let changes = [change("a.txt"), change("css/b.css"), change("c.md")];
        assert_eq!(pick_row(None, &changes), Some(0));
        assert_eq!(pick_row(Some("css/b.css"), &changes), Some(1));
        // It moved in the list.
        assert_eq!(pick_row(Some("css/b.css"), &changes[1..]), Some(0));
        // It is gone (saved, thrown away): the first file.
        assert_eq!(pick_row(Some("gone.txt"), &changes), Some(0));
        assert_eq!(pick_row(Some("a.txt"), &[]), None);
        assert_eq!(pick_row(None, &[]), None);
    }
}
