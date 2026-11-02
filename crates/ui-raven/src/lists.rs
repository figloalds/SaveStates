//! List views, tree views, tab views and combo boxes.

/// List views (`<ListView>`).
pub mod list {
    use crate::widget::Widget;
    use crate::window::Window;
    use crate::{cstr, index, leak, to_int, with_callback};
    use raven::{
        R_EVENT_MOUSE_DOWN, R_EVENT_MOUSE_UP, R_INVALID_ID, R_MOUSE_LEFT, R_MOUSE_RIGHT, REvent,
        Rid,
    };
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::rc::Rc;

    type SelectionFn = Box<dyn FnMut(Option<usize>)>;

    /// The rows of a virtual list. One per list, registered with Raven once and never freed;
    /// [`set_virtual`] swaps the closure inside.
    struct VirtualRows {
        text: RefCell<Rc<dyn Fn(usize) -> String>>,
        /// The last text handed to Raven, which must stay valid until its next call.
        last: RefCell<CString>,
    }

    thread_local! {
        static VIRTUAL_ROWS: RefCell<HashMap<Rid, &'static VirtualRows>> = RefCell::new(HashMap::new());
    }

    /// Shows `count` rows whose texts the list asks `text` for, only for the rows it draws.
    /// Call it again when the rows change: the new closure replaces the previous one.
    pub fn set_virtual(w: Widget, count: usize, text: impl Fn(usize) -> String + 'static) {
        if !crate::is_ui_thread() {
            return;
        }
        let text: Rc<dyn Fn(usize) -> String> = Rc::new(text);
        let rows = VIRTUAL_ROWS.with_borrow_mut(|lists| match lists.get(&w.0) {
            Some(rows) => {
                // The trampoline never holds this borrow while the closure runs.
                *rows.text.borrow_mut() = text;
                *rows
            }
            None => {
                let rows: &'static VirtualRows = Box::leak(Box::new(VirtualRows {
                    text: RefCell::new(text),
                    last: RefCell::new(CString::default()),
                }));
                lists.insert(w.0, rows);
                rows
            }
        });
        let user = std::ptr::from_ref(rows).cast_mut().cast::<c_void>();
        // SAFETY: `user` points to a `VirtualRows` that is never freed.
        unsafe {
            raven::Rlist_view_set_virtual_items(w.0, to_int(count), Some(text_trampoline), user)
        };
    }

    unsafe extern "C" fn text_trampoline(_list: Rid, row: i32, user: *mut c_void) -> *const c_char {
        // SAFETY: Raven passes back the `VirtualRows` given in `set_virtual`, on the UI thread.
        let rows = unsafe { &*user.cast::<VirtualRows>() };
        // A clone keeps the closure alive even if it calls `set_virtual` on its own list.
        let text = rows.text.borrow().clone();
        let s = index(row).map(|i| text(i)).unwrap_or_default();
        let mut last = rows.last.borrow_mut();
        *last = cstr(&s);
        last.as_ptr()
    }

    /// The selected row.
    pub fn selected(w: Widget) -> Option<usize> {
        // SAFETY: plain call.
        index(unsafe { raven::Rlist_view_get_selected(w.0) })
    }

    /// Selects a row, or nothing.
    pub fn set_selected(w: Widget, i: Option<usize>) {
        let i = i.map_or(-1, to_int);
        // SAFETY: plain call.
        unsafe { raven::Rlist_view_set_selected(w.0, i) };
    }

    /// Called when the selection changes, with the selected row.
    pub fn on_selection(w: Widget, f: impl FnMut(Option<usize>) + 'static) {
        let Some(user) = leak::<SelectionFn>(Box::new(f)) else {
            return;
        };
        // SAFETY: `user` is a `SelectionFn` box that is never freed.
        unsafe {
            raven::Rlist_view_set_selection_changed_callback(w.0, Some(selection_trampoline), user)
        };
    }

    unsafe extern "C" fn selection_trampoline(_list: Rid, row: i32, user: *mut c_void) {
        // SAFETY: Raven passes back the `SelectionFn` box given in `on_selection`, on the UI thread.
        unsafe { with_callback::<SelectionFn, _>(user, |f| f(index(row))) };
    }

    thread_local! {
        /// The context menu of each list given one by [`set_context_menu`].
        static ROW_MENUS: RefCell<HashMap<Rid, Rid>> = RefCell::new(HashMap::new());
        /// The windows whose input [`row_menu_hook`] watches.
        static HOOKED: RefCell<HashSet<Rid>> = RefCell::new(HashSet::new());
        /// A right press on a list was taken: its release is taken too.
        static TAKEN: Cell<bool> = const { Cell::new(false) };
    }

    /// Opens `menu` (a `<Menu>` made with [`crate::widget_from_xml`]) when the user right-clicks
    /// the list `w` of `window`. A right click on a row selects that row first, as a click
    /// does (and `on_selection` hears of it as of a click), so what the menu does applies to
    /// the row under the pointer, as in a file manager.
    ///
    /// Raven's own context menus leave a list's selection alone, so this watches the window's
    /// input instead (with its event hook, which nothing else may set): it takes the right
    /// click, then on the next frame clicks the left button there and opens the menu. While a
    /// menu or a dialog is open, a right click is left to Raven.
    pub fn set_context_menu(window: Window, w: Widget, menu: Widget) {
        if !crate::is_ui_thread() {
            return;
        }
        ROW_MENUS.with_borrow_mut(|menus| menus.insert(w.0, menu.0));
        if HOOKED.with_borrow_mut(|hooked| hooked.insert(window.0)) {
            // SAFETY: the hook uses no user data.
            unsafe {
                raven::Rwindow_set_event_hook(window.0, Some(row_menu_hook), std::ptr::null_mut())
            };
        }
    }

    unsafe extern "C" fn row_menu_hook(
        _window: Rid,
        event: *const REvent,
        _user: *mut c_void,
    ) -> c_int {
        // SAFETY: Raven passes a valid event, or null, for the duration of the call.
        let Some(e) = (unsafe { event.as_ref() }) else {
            return 0;
        };
        if e.button != R_MOUSE_RIGHT {
            return 0;
        }
        match e.type_ {
            R_EVENT_MOUSE_DOWN => {
                // Under an open menu or dialog, the target is never the list: the click is
                // Raven's.
                let Some(menu) = menu_under(e.target) else {
                    return 0;
                };
                TAKEN.set(true);
                let (window, x, y) = (e.window, e.x, e.y);
                // Not from inside the hook: the click is handled at once, as the user's would be.
                crate::task::post(move || {
                    // SAFETY: plain calls; Raven checks the handles (the window or the menu may
                    // be gone by now).
                    unsafe {
                        raven::Rwindow_inject_mouse_move(window, x, y);
                        raven::Rwindow_inject_mouse_button(window, R_MOUSE_LEFT, 1);
                        raven::Rwindow_inject_mouse_button(window, R_MOUSE_LEFT, 0);
                        raven::Rmenu_open(menu, window, x, y);
                    }
                });
                1
            }
            R_EVENT_MOUSE_UP if TAKEN.replace(false) => 1,
            _ => 0,
        }
    }

    /// The context menu of the list that `target` is, or is inside of.
    fn menu_under(target: Rid) -> Option<Rid> {
        let mut widget = target;
        while widget != R_INVALID_ID {
            if let Some(menu) = ROW_MENUS.with_borrow(|menus| menus.get(&widget).copied()) {
                return Some(menu);
            }
            // SAFETY: plain call; Raven checks the handle.
            widget = unsafe { raven::Rwidget_get_parent(widget) };
        }
        None
    }
}

/// Tree views (`<TreeView>`).
pub mod tree {
    use crate::widget::Widget;
    use crate::{cstr, leak, with_callback};
    use raven::{R_INVALID_ID, Rid};
    use std::ffi::c_void;

    type SelectionFn = Box<dyn FnMut(Option<TreeItem>)>;

    /// An item of a tree view.
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
    pub struct TreeItem(pub(crate) Rid);

    /// Deletes every item.
    pub fn clear(w: Widget) {
        // SAFETY: plain call.
        unsafe { raven::Rtree_view_clear_items(w.0) };
    }

    /// Adds an item under `parent`, or at the top level.
    pub fn add(w: Widget, parent: Option<TreeItem>, text: &str) -> TreeItem {
        let text = cstr(text);
        let parent = parent.map_or(R_INVALID_ID, |p| p.0);
        // SAFETY: `text` outlives the call.
        TreeItem(unsafe { raven::Rtree_view_add_item(w.0, parent, text.as_ptr()) })
    }

    /// Opens (`true`) or closes the branch of an item, showing or hiding what is under it.
    pub fn set_expanded(item: TreeItem, on: bool) {
        // SAFETY: plain call; Raven checks the handle.
        unsafe { raven::Rtree_item_set_expanded(item.0, i32::from(on)) };
    }

    /// Called when the selection changes, with the selected item.
    pub fn on_selection(w: Widget, f: impl FnMut(Option<TreeItem>) + 'static) {
        let Some(user) = leak::<SelectionFn>(Box::new(f)) else {
            return;
        };
        // SAFETY: `user` is a `SelectionFn` box that is never freed.
        unsafe {
            raven::Rtree_view_set_selection_changed_callback(w.0, Some(selection_trampoline), user)
        };
    }

    unsafe extern "C" fn selection_trampoline(_tree: Rid, item: Rid, user: *mut c_void) {
        let item = (item != R_INVALID_ID).then_some(TreeItem(item));
        // SAFETY: Raven passes back the `SelectionFn` box given in `on_selection`, on the UI thread.
        unsafe { with_callback::<SelectionFn, _>(user, |f| f(item)) };
    }
}

/// Tab views (`<TabView>`): one page per tab.
pub mod tabs {
    use crate::widget::Widget;
    use crate::{cstr, index, leak, to_int, with_callback};
    use raven::Rid;
    use std::ffi::c_void;

    type ChangedFn = Box<dyn FnMut(usize)>;
    type CloseFn = Box<dyn FnMut(Widget) -> bool>;

    /// Adds a tab showing `content` and returns its index (`usize::MAX` if Raven refused it,
    /// for an invalid handle).
    pub fn add(w: Widget, title: &str, content: Widget) -> usize {
        let title = cstr(title);
        // SAFETY: `title` outlives the call.
        index(unsafe { raven::Rtab_view_add_tab(w.0, title.as_ptr(), content.0) })
            .unwrap_or(usize::MAX)
    }

    pub fn set_active(w: Widget, i: usize) {
        // SAFETY: plain call.
        unsafe { raven::Rtab_view_set_active(w.0, to_int(i)) };
    }

    /// Renames the tab that shows `content`.
    pub fn set_title(w: Widget, content: Widget, title: &str) {
        let title = cstr(title);
        // SAFETY: `title` outlives the call.
        unsafe { raven::Rtab_view_set_tab_title(w.0, content.0, title.as_ptr()) };
    }

    /// Shows or hides the close button of the tab that shows `content`.
    pub fn set_closable(w: Widget, content: Widget, on: bool) {
        // SAFETY: plain call.
        unsafe { raven::Rtab_view_set_tab_closable(w.0, content.0, i32::from(on)) };
    }

    /// Called when another tab becomes the active one, with its index.
    pub fn on_changed(w: Widget, f: impl FnMut(usize) + 'static) {
        let Some(user) = leak::<ChangedFn>(Box::new(f)) else {
            return;
        };
        // SAFETY: `user` is a `ChangedFn` box that is never freed.
        unsafe { raven::Rtab_view_set_changed_callback(w.0, Some(changed_trampoline), user) };
    }

    unsafe extern "C" fn changed_trampoline(_tabs: Rid, i: i32, user: *mut c_void) {
        let Some(i) = index(i) else { return };
        // SAFETY: Raven passes back the `ChangedFn` box given in `on_changed`, on the UI thread.
        unsafe { with_callback::<ChangedFn, _>(user, |f| f(i)) };
    }

    /// Called when the user closes a tab, with its page: true deletes the page now, false keeps
    /// the tab.
    pub fn on_close(w: Widget, f: impl FnMut(Widget) -> bool + 'static) {
        let Some(user) = leak::<CloseFn>(Box::new(f)) else {
            return;
        };
        // SAFETY: `user` is a `CloseFn` box that is never freed.
        unsafe { raven::Rtab_view_set_close_callback(w.0, Some(close_trampoline), user) };
    }

    unsafe extern "C" fn close_trampoline(
        _tabs: Rid,
        _i: i32,
        content: Rid,
        user: *mut c_void,
    ) -> i32 {
        // SAFETY: Raven passes back the `CloseFn` box given in `on_close`, on the UI thread.
        // A nested call keeps the tab.
        let close = unsafe { with_callback::<CloseFn, _>(user, |f| f(Widget(content))) };
        i32::from(close.unwrap_or(false))
    }
}

/// Combo boxes (`<ComboBox>`).
pub mod combo {
    use crate::widget::Widget;
    use crate::{cstr, index, leak, to_int, with_callback};
    use raven::Rid;
    use std::ffi::c_void;

    type ChangedFn = Box<dyn FnMut(usize)>;

    /// Replaces the items.
    pub fn set_items(w: Widget, items: &[&str]) {
        // SAFETY: plain call.
        unsafe { raven::Rcombo_box_clear_items(w.0) };
        for item in items {
            let item = cstr(item);
            // SAFETY: `item` outlives the call.
            unsafe { raven::Rcombo_box_add_item(w.0, item.as_ptr()) };
        }
    }

    /// The selected item.
    pub fn selected(w: Widget) -> Option<usize> {
        // SAFETY: plain call.
        index(unsafe { raven::Rcombo_box_get_selected(w.0) })
    }

    pub fn set_selected(w: Widget, i: usize) {
        // SAFETY: plain call.
        unsafe { raven::Rcombo_box_set_selected(w.0, to_int(i)) };
    }

    /// Called when the selected item changes, with its index.
    pub fn on_changed(w: Widget, f: impl FnMut(usize) + 'static) {
        let Some(user) = leak::<ChangedFn>(Box::new(f)) else {
            return;
        };
        // SAFETY: `user` is a `ChangedFn` box that is never freed.
        unsafe { raven::Rcombo_box_set_changed_callback(w.0, Some(changed_trampoline), user) };
    }

    unsafe extern "C" fn changed_trampoline(_combo: Rid, i: i32, user: *mut c_void) {
        let Some(i) = index(i) else { return };
        // SAFETY: Raven passes back the `ChangedFn` box given in `on_changed`, on the UI thread.
        unsafe { with_callback::<ChangedFn, _>(user, |f| f(i)) };
    }
}
