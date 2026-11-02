//! Peek (spec §6.5): a past save in the lower panels, without touching the files on disk.
//!
//! A banner names the save and its date, with "Bring back everything" and "Back to Now". Two
//! tabs replace the list of unsaved changes: "What changed" (what the save changed compared with
//! the save before it) and "All files" (every file in the save, as a folder tree with a filter).
//! The tab ([`crate::tabs`]) decides what the comparison view shows for each.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use chrono::Local;
use savestates_core::{Change, SaveId, SaveState};
use savestates_i18n::{Lang, relative_date};
use savestates_ui_raven::{TreeItem, Widget, tabs, tr, tr_fmt, tree};

use crate::app::with_tab;
use crate::changes_panel::ChangesPanel;
use crate::files_tree::{TreeNode, build_tree, filter_paths};
use crate::tabs::TabId;

/// The two tabs of peek mode, in their order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeekTab {
    Changed,
    AllFiles,
}

/// What the user did in peek mode, for the tab to act on.
pub enum PeekEvent {
    /// Selected a file in "What changed".
    ChangeSelected(Change),
    /// Selected an item (a file or a folder) of the "All files" tree, or nothing.
    ItemSelected(Option<TreeItem>),
    /// Switched to another tab.
    TabChanged(PeekTab),
    /// Typed in the filter box of "All files".
    Filter,
}

/// The save peeked at.
pub struct Peeked {
    pub save: SaveState,
    /// The name of the save just before it, when that one is loaded on the timeline.
    pub before_name: Option<String>,
    /// Every file of the save, once read.
    files: Option<Vec<String>>,
    /// The file selected in "All files".
    pub file: Option<String>,
}

/// Peek mode in one tab: its widgets (`project_tab.xml`) and the save it shows.
pub struct PeekView {
    banner: Widget,
    banner_text: Widget,
    /// "Bring back everything".
    pub everything: Widget,
    /// "Back to Now".
    pub back: Widget,
    lists: Widget,
    /// The pages of the two tabs, whose titles are set in code.
    pages: [Widget; 2],
    /// "What changed".
    pub changes: ChangesPanel,
    filter: Widget,
    tree: Widget,
    /// The file each item of the tree stands for (folders have none).
    items: RefCell<HashMap<TreeItem, String>>,
    tab: Cell<PeekTab>,
    peeked: RefCell<Option<Peeked>>,
}

impl PeekView {
    /// Finds peek's widgets in the tab's `page` and reports what the user does in them to the
    /// tab `id`.
    pub fn new(page: Widget, id: TabId) -> Result<PeekView, String> {
        let find = |name: &str| {
            page.find(name)
                .ok_or_else(|| format!("project_tab.xml has no widget named {name}"))
        };
        let lists = find("peek.tabs")?;
        let pages = [find("peek.changed_page")?, find("peek.files_page")?];
        set_tab_titles(lists, pages);
        tabs::on_changed(lists, move |i| {
            let tab = if i == 0 {
                PeekTab::Changed
            } else {
                PeekTab::AllFiles
            };
            with_tab(id, |t| t.peek_event(PeekEvent::TabChanged(tab)));
        });
        let changes = ChangesPanel::new(find("peek.changes_host")?, move |change| {
            with_tab(id, |t| t.peek_event(PeekEvent::ChangeSelected(change)));
        });
        let filter = find("peek.filter")?;
        filter.on_text_changed(move |_| {
            with_tab(id, |t| t.peek_event(PeekEvent::Filter));
        });
        let tree_view = find("peek.tree")?;
        tree::on_selection(tree_view, move |item| {
            with_tab(id, |t| t.peek_event(PeekEvent::ItemSelected(item)));
        });
        Ok(PeekView {
            banner: find("peek.banner")?,
            banner_text: find("peek.banner_text")?,
            everything: find("peek.everything")?,
            back: find("peek.back")?,
            lists,
            pages,
            changes,
            filter,
            tree: tree_view,
            items: RefCell::default(),
            tab: Cell::new(PeekTab::Changed),
            peeked: RefCell::default(),
        })
    }

    /// Starts peeking at `save` (the one before it is named `before_name`): the banner and the
    /// tabs show, on "What changed", empty until [`PeekView::loaded`].
    pub fn enter(&self, save: SaveState, before_name: Option<String>, lang: Lang) {
        self.banner_text.set_label_text(&banner_text(&save, lang));
        *self.peeked.borrow_mut() = Some(Peeked {
            save,
            before_name,
            files: None,
            file: None,
        });
        self.changes.set_changes(Vec::new());
        self.clear_tree();
        self.filter.text_input_set("");
        self.tab.set(PeekTab::Changed);
        tabs::set_active(self.lists, 0);
        self.banner.set_visible(true);
        self.lists.set_visible(true);
    }

    /// Shows the texts set in code in the current language, with dates in `lang`: the tab
    /// titles, the banner and the badges of "What changed".
    pub fn relabel(&self, lang: Lang) {
        set_tab_titles(self.lists, self.pages);
        if let Some(peeked) = self.peeked.borrow().as_ref() {
            self.banner_text
                .set_label_text(&banner_text(&peeked.save, lang));
        }
        self.changes.relabel();
    }

    /// Stops peeking: the banner and the tabs go away.
    pub fn leave(&self) {
        *self.peeked.borrow_mut() = None;
        self.banner.set_visible(false);
        self.lists.set_visible(false);
        self.changes.set_changes(Vec::new());
        self.clear_tree();
    }

    /// The save peeked at, if any.
    pub fn peeked_id(&self) -> Option<SaveId> {
        self.peeked.borrow().as_ref().map(|p| p.save.id.clone())
    }

    /// Runs `f` with the save peeked at, if any.
    pub fn with_peeked<R>(&self, f: impl FnOnce(&Peeked) -> R) -> Option<R> {
        self.peeked.borrow().as_ref().map(f)
    }

    pub fn tab(&self) -> PeekTab {
        self.tab.get()
    }

    /// The user switched to `tab`. "All files" fills its tree the first time it shows.
    pub fn set_tab(&self, tab: PeekTab) {
        self.tab.set(tab);
        if tab == PeekTab::AllFiles && self.items.borrow().is_empty() {
            self.show_tree();
        }
    }

    /// What the save `id` changed and every file in it were read. Returns the change selected
    /// in "What changed", or `None` (also when peek moved on to another save meanwhile).
    pub fn loaded(&self, id: &SaveId, changes: Vec<Change>, files: Vec<String>) -> Option<Change> {
        {
            let mut peeked = self.peeked.borrow_mut();
            let peeked = peeked.as_mut().filter(|p| p.save.id == *id)?;
            peeked.files = Some(files);
        }
        if self.tab.get() == PeekTab::AllFiles {
            self.show_tree();
        }
        self.changes.set_changes(changes)
    }

    /// The user typed in the filter box: the tree shows the files that match, with their
    /// folders open.
    pub fn filter(&self) {
        self.show_tree();
    }

    /// The user selected `item` in the tree: returns the file it stands for (`None` for a
    /// folder), now the selected file of "All files".
    pub fn select_item(&self, item: Option<TreeItem>) -> Option<String> {
        let file = item.and_then(|item| self.items.borrow().get(&item).cloned());
        if let Some(peeked) = self.peeked.borrow_mut().as_mut() {
            peeked.file.clone_from(&file);
        }
        file
    }

    /// Fills the tree with the save's files that match the filter. Every folder is open while
    /// a filter is typed, so the matches show; otherwise they start closed.
    fn show_tree(&self) {
        self.clear_tree();
        let query = self.filter.text_input_get();
        let nodes = {
            let peeked = self.peeked.borrow();
            let Some(files) = peeked.as_ref().and_then(|p| p.files.as_ref()) else {
                return;
            };
            build_tree(&filter_paths(files, query.trim()))
        };
        let mut items = self.items.borrow_mut();
        add_nodes(
            self.tree,
            None,
            &nodes,
            !query.trim().is_empty(),
            &mut items,
        );
    }

    fn clear_tree(&self) {
        tree::clear(self.tree);
        self.items.borrow_mut().clear();
    }
}

/// Titles the tabs "What changed" and "All files" (`pages`, in that order). Raven translates no
/// `Slot.*` attribute, so the titles are set in code.
fn set_tab_titles(lists: Widget, pages: [Widget; 2]) {
    tabs::set_title(lists, pages[0], &tr("peek.tab.changed"));
    tabs::set_title(lists, pages[1], &tr("peek.tab.all_files"));
}

/// The banner's text: "Espiando Modo escuro · ontem".
fn banner_text(save: &SaveState, lang: Lang) -> String {
    let date = relative_date(lang, save.time, Local::now().fixed_offset());
    tr_fmt("peek.banner", &[&save.name, &date])
}

/// Adds `nodes` under `parent` (the top level for `None`), recording each file's item.
fn add_nodes(
    view: Widget,
    parent: Option<TreeItem>,
    nodes: &[TreeNode],
    open: bool,
    items: &mut HashMap<TreeItem, String>,
) {
    for node in nodes {
        let item = tree::add(view, parent, &node.name);
        match &node.path {
            Some(path) => {
                items.insert(item, path.clone());
            }
            None => {
                add_nodes(view, Some(item), &node.children, open, items);
                if open {
                    tree::set_expanded(item, true);
                }
            }
        }
    }
}
