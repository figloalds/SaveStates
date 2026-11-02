//! Widgets and actions.

use crate::{clear_last_error, cstr, last_error, leak, text, with_callback};
use raven::{R_COLLAPSED, R_INVALID_ID, R_VISIBLE, Rid};
use std::ffi::{c_char, c_int, c_void};
use std::ptr;

/// A widget of a window, or one made from XML that is not in a window yet.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Widget(pub(crate) Rid);

type ActionFn = Box<dyn FnMut(Widget)>;
type TextFn = Box<dyn FnMut(String)>;

/// Builds a widget tree from a document whose root element is a widget (for dialogs and tab
/// pages). The error is Raven's text.
pub fn widget_from_xml(xml: &str) -> Result<Widget, String> {
    if xml.is_empty() {
        return Err("the widget XML is empty".to_owned());
    }
    clear_last_error();
    // SAFETY: the text and its length describe a live buffer; Raven does not need a NUL.
    let id = unsafe { raven::Rwidget_create_from_xml(xml.as_ptr().cast(), xml.len()) };
    if id == R_INVALID_ID {
        Err(last_error())
    } else {
        Ok(Widget(id))
    }
}

/// Binds an action name (`OnClicked="name"` in XML) to a closure, which receives the widget
/// that fired it. Binding the name again replaces the closure.
pub fn bind_action(name: &str, f: impl FnMut(Widget) + 'static) {
    let Some(user) = leak::<ActionFn>(Box::new(f)) else {
        return;
    };
    let name = cstr(name);
    // SAFETY: `name` outlives the call (Raven copies it); `user` is an `ActionFn` box that is
    // never freed.
    unsafe { raven::Raction_bind(name.as_ptr(), Some(action_trampoline), user) };
}

unsafe extern "C" fn action_trampoline(sender: Rid, _action: *const c_char, user: *mut c_void) {
    // SAFETY: Raven passes back the `ActionFn` box given in `bind_action`, on the UI thread.
    unsafe { with_callback::<ActionFn, _>(user, |f| f(Widget(sender))) };
}

unsafe extern "C" fn text_trampoline(_input: Rid, s: *const c_char, user: *mut c_void) {
    // SAFETY: Raven passes a copy of the text, valid for the duration of the call.
    let s = unsafe { text(s) };
    // SAFETY: Raven passes back the `TextFn` box given in `on_text_changed`, on the UI thread.
    unsafe { with_callback::<TextFn, _>(user, |f| f(s)) };
}

impl Widget {
    /// The widget named `name` (`Name="..."`) inside this one (itself included). Unlike
    /// [`crate::Window::find`], this finds widgets of a tree made from XML before it is put in a
    /// window, and tells apart widgets of the same name in different trees (one tree per tab).
    pub fn find(&self, name: &str) -> Option<Widget> {
        let name = cstr(name);
        // SAFETY: `name` outlives the call.
        let id = unsafe { raven::Rwidget_find(self.0, name.as_ptr()) };
        (id != R_INVALID_ID).then_some(Widget(id))
    }

    /// Deletes the widget and everything inside it; a tab page leaves its tab view. Callbacks
    /// given to them are not freed (see the crate documentation).
    pub fn destroy(&self) {
        // SAFETY: plain call; Raven checks the handle.
        unsafe { raven::Rwidget_destroy(self.0) };
    }

    pub fn set_label_text(&self, s: &str) {
        let s = cstr(s);
        // SAFETY: `s` outlives the call.
        unsafe { raven::Rlabel_set_text(self.0, s.as_ptr()) };
    }

    pub fn set_button_text(&self, s: &str) {
        let s = cstr(s);
        // SAFETY: `s` outlives the call.
        unsafe { raven::Rbutton_set_text(self.0, s.as_ptr()) };
    }

    /// The text of a text input.
    pub fn text_input_get(&self) -> String {
        // SAFETY: Raven returns null or a text valid until the input changes; it is copied at once.
        unsafe { text(raven::Rtext_input_get_text(self.0)) }
    }

    pub fn text_input_set(&self, s: &str) {
        let s = cstr(s);
        // SAFETY: `s` outlives the call.
        unsafe { raven::Rtext_input_set_text(self.0, s.as_ptr()) };
    }

    /// Selects all the text of a text input, so that typing replaces it.
    pub fn text_input_select_all(&self) {
        // SAFETY: plain call.
        unsafe { raven::Rtext_input_select_all(self.0) };
    }

    /// How far a progress bar is filled, from 0 to 1.
    pub fn set_progress(&self, value: f32) {
        // SAFETY: plain call.
        unsafe { raven::Rprogress_bar_set_value(self.0, value) };
    }

    /// Gives the keyboard to the widget (a dialog does not give it to any widget by itself).
    pub fn focus(&self) {
        // SAFETY: plain call; Raven checks the handle.
        unsafe { raven::Rwidget_focus(self.0) };
    }

    /// Called after every edit the user makes in a text input, with its text. Setting the text
    /// from code ([`Widget::text_input_set`]) does not call it. Setting it again replaces the
    /// previous closure.
    pub fn on_text_changed(&self, f: impl FnMut(String) + 'static) {
        let Some(user) = leak::<TextFn>(Box::new(f)) else {
            return;
        };
        // SAFETY: `user` is a `TextFn` box that is never freed.
        unsafe { raven::Rtext_input_set_changed_callback(self.0, Some(text_trampoline), user) };
    }

    /// A disabled widget ignores input and is drawn dimmed, with what it contains.
    pub fn set_enabled(&self, on: bool) {
        // SAFETY: plain call.
        unsafe { raven::Rwidget_set_enabled(self.0, c_int::from(on)) };
    }

    /// A hidden widget is not drawn and takes no space (collapsed).
    pub fn set_visible(&self, on: bool) {
        let visibility = if on { R_VISIBLE } else { R_COLLAPSED };
        // SAFETY: plain call.
        unsafe { raven::Rwidget_set_visibility(self.0, visibility) };
    }

    /// Text shown when the pointer rests on the widget; `None` removes it.
    pub fn set_tooltip(&self, s: Option<&str>) {
        let s = s.map(cstr);
        let pointer = s.as_ref().map_or(ptr::null(), |s| s.as_ptr());
        // SAFETY: `pointer` is null or points into `s`, which outlives the call.
        unsafe { raven::Rwidget_set_tooltip(self.0, pointer) };
    }

    /// Appends `child` (made in code, as by [`crate::custom::create`]) to this panel, which
    /// takes it. A child that already has a parent is moved. In an `Overlay` or a `Border` the
    /// child fills the panel.
    pub fn add_child(&self, child: Widget) {
        // SAFETY: plain call; Raven checks both handles.
        unsafe { raven::Rwidget_add_child(self.0, child.0) };
    }

    /// Asks for the widget to be laid out and painted again.
    pub fn invalidate(&self) {
        // SAFETY: plain call.
        unsafe { raven::Rwidget_invalidate(self.0) };
    }

    /// Bounds in the window (x, y, width, height), in logical units, after the last layout.
    pub fn rect(&self) -> (f32, f32, f32, f32) {
        let (mut x, mut y, mut w, mut h) = (0.0, 0.0, 0.0, 0.0);
        // SAFETY: all four outputs are valid.
        unsafe { raven::Rwidget_get_rect(self.0, &mut x, &mut y, &mut w, &mut h) };
        (x, y, w, h)
    }

    /// Makes a text attribute ("Text", "Tooltip", "Placeholder", "Title"...) show the
    /// translation of `key`, now and each time the language changes.
    pub fn set_text_key(&self, attr: &str, key: &str) {
        let (attr, key) = (cstr(attr), cstr(key));
        // SAFETY: both texts outlive the call.
        unsafe { raven::Rwidget_set_text_key(self.0, attr.as_ptr(), key.as_ptr()) };
    }
}
