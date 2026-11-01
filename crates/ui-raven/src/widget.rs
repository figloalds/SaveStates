//! Widgets and actions.

use crate::{clear_last_error, cstr, last_error, leak, text, with_callback};
use raven::{R_COLLAPSED, R_INVALID_ID, R_VISIBLE, Rid};
use std::ffi::{c_char, c_int, c_void};
use std::ptr;

/// A widget of a window, or one made from XML that is not in a window yet.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Widget(pub(crate) Rid);

type ActionFn = Box<dyn FnMut(Widget)>;

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

impl Widget {
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
