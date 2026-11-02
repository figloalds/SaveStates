//! A list of changed files (spec §6.3): the unsaved changes while "Now" is selected, and what a
//! peeked save changed. One row per file, with its name and a badge (changed, new, deleted,
//! renamed); selecting a row shows that file in the comparison view.

use std::cell::RefCell;
use std::rc::Rc;

use savestates_core::{Change, ChangeKind};
use savestates_ui_raven::{Widget, list, tr};

use crate::tr_key;

/// The changes shown in a virtual `ListView`, which asks only for the rows it draws.
pub struct ChangesPanel {
    list: Widget,
    changes: Rc<RefCell<Vec<Change>>>,
}

impl ChangesPanel {
    /// Shows changes in `list` (none yet). `on_select` gets the change the user selects; a
    /// selection made by [`ChangesPanel::set_changes`] is not reported.
    pub fn new(list: Widget, mut on_select: impl FnMut(Change) + 'static) -> ChangesPanel {
        let changes: Rc<RefCell<Vec<Change>>> = Rc::default();
        list::on_selection(list, {
            let changes = changes.clone();
            move |row| {
                let change = row.and_then(|i| changes.borrow().get(i).cloned());
                if let Some(change) = change {
                    on_select(change);
                }
            }
        });
        let panel = ChangesPanel { list, changes };
        panel.set_changes(Vec::new());
        panel
    }

    /// Shows `changes`. The file that was selected stays selected while it is among them,
    /// otherwise the first one is. Returns the selected change.
    pub fn set_changes(&self, changes: Vec<Change>) -> Option<Change> {
        let was = self.selected().map(|change| change.path);
        let row = pick_row(was.as_deref(), &changes);
        let count = changes.len();
        *self.changes.borrow_mut() = changes;
        list::set_virtual(self.list, count, {
            let changes = self.changes.clone();
            move |i| changes.borrow().get(i).map(row_text).unwrap_or_default()
        });
        list::set_selected(self.list, row);
        self.selected()
    }

    /// Selects the change of the file at `path` and returns it; `None` (and nothing changes)
    /// when that file has no change shown. Like any selection made in code, it is not reported.
    pub fn select_path(&self, path: &str) -> Option<Change> {
        let row = self.changes.borrow().iter().position(|c| c.path == path)?;
        list::set_selected(self.list, Some(row));
        self.selected()
    }

    /// The selected change.
    pub fn selected(&self) -> Option<Change> {
        let row = list::selected(self.list)?;
        self.changes.borrow().get(row).cloned()
    }

    /// The changes shown.
    pub fn changes(&self) -> Vec<Change> {
        self.changes.borrow().clone()
    }
}

/// Which row to select among `changes`: the one of the file at `was` (the path selected before)
/// while it is there, else the first. `None` when there are no changes.
fn pick_row(was: Option<&str>, changes: &[Change]) -> Option<usize> {
    let kept = was.and_then(|path| changes.iter().position(|change| change.path == path));
    kept.or((!changes.is_empty()).then_some(0))
}

/// A row: "estilo.css  ·  alterado", and the folder after it for a file in one.
fn row_text(change: &Change) -> String {
    let badge = tr(badge_key(&change.kind));
    match change.path.rsplit_once('/') {
        Some((folder, name)) => format!("{name}  ·  {badge}  ·  {folder}/"),
        None => format!("{}  ·  {badge}", change.path),
    }
}

/// The text key of a change's badge.
fn badge_key(kind: &ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Changed => tr_key("change.changed"),
        ChangeKind::New => tr_key("change.new"),
        ChangeKind::Deleted => tr_key("change.deleted"),
        ChangeKind::Renamed { .. } => tr_key("change.renamed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn every_kind_of_change_has_its_badge() {
        assert_eq!(badge_key(&ChangeKind::Changed), "change.changed");
        assert_eq!(badge_key(&ChangeKind::New), "change.new");
        assert_eq!(badge_key(&ChangeKind::Deleted), "change.deleted");
        let renamed = ChangeKind::Renamed {
            from: "a".to_owned(),
        };
        assert_eq!(badge_key(&renamed), "change.renamed");
    }
}
