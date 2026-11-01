//! Top-level windows.

use crate::widget::Widget;
use crate::{clear_last_error, cstr, is_ui_thread, last_error, leak, with_callback};
use raven::{R_INVALID_ID, R_KEY_LEFT_CTRL, R_MOD_CTRL, R_MOUSE_LEFT, Rid};
use std::cell::{Cell, RefCell};
use std::ffi::{c_int, c_void};
use std::rc::Rc;

/// A top-level window.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Window(pub(crate) Rid);

type CloseFn = Box<dyn FnMut() -> bool>;

impl Window {
    /// Opens a window from a document whose root element is `<Window>`. The error is Raven's text.
    pub fn from_xml(xml: &str) -> Result<Window, String> {
        if xml.is_empty() {
            return Err("the window XML is empty".to_owned());
        }
        clear_last_error();
        // SAFETY: the text and its length describe a live buffer; Raven does not need a NUL.
        let id = unsafe { raven::Rwindow_create_from_xml(xml.as_ptr().cast(), xml.len()) };
        if id == R_INVALID_ID {
            Err(last_error())
        } else {
            Ok(Window(id))
        }
    }

    /// The widget named `name` (`Name="..."`) anywhere in the window.
    pub fn find(&self, name: &str) -> Option<Widget> {
        let name = cstr(name);
        // SAFETY: `name` outlives the call.
        let id = unsafe { raven::Rwidget_find(self.0, name.as_ptr()) };
        (id != R_INVALID_ID).then_some(Widget(id))
    }

    /// Client size, in logical units.
    pub fn size(&self) -> (i32, i32) {
        let (mut w, mut h) = (0, 0);
        // SAFETY: both outputs are valid.
        unsafe { raven::Rwindow_get_size(self.0, &mut w, &mut h) };
        (w, h)
    }

    pub fn set_size(&self, w: i32, h: i32) {
        // SAFETY: plain call.
        unsafe { raven::Rwindow_set_size(self.0, w, h) };
    }

    /// Position on the screen.
    pub fn position(&self) -> (i32, i32) {
        let (mut x, mut y) = (0, 0);
        // SAFETY: both outputs are valid.
        unsafe { raven::Rwindow_get_position(self.0, &mut x, &mut y) };
        (x, y)
    }

    pub fn is_maximized(&self) -> bool {
        // SAFETY: plain call.
        unsafe { raven::Rwindow_is_maximized(self.0) != 0 }
    }

    /// Called when the user asks to close the window: true lets it close, false keeps it open.
    pub fn on_close(&self, f: impl FnMut() -> bool + 'static) {
        let Some(user) = leak::<CloseFn>(Box::new(f)) else {
            return;
        };
        // SAFETY: `user` is a `CloseFn` box that is never freed.
        unsafe { raven::Rwindow_set_close_callback(self.0, Some(close_trampoline), user) };
    }

    /// Called each time the window becomes the one the user works in again (after another
    /// application had the keyboard).
    ///
    /// Raven tells no event for this (its window event hook sees only input events), so a frame
    /// callback watches `Rwindow_is_active`: activating a window wakes the frame loop.
    pub fn on_focus_gained(&self, f: impl FnMut() + 'static) {
        if !is_ui_thread() {
            return;
        }
        // SAFETY: plain call.
        let active = unsafe { raven::Rwindow_is_active(self.0) != 0 };
        FOCUS_WATCHES.with_borrow_mut(|watches| {
            watches.push(Rc::new(FocusWatch {
                window: self.0,
                was_active: Cell::new(active),
                callback: RefCell::new(Box::new(f)),
            }))
        });
        // Setting the same callback again is harmless, and survives a shutdown and a new init.
        // SAFETY: the callback uses no user data.
        unsafe { raven::Rset_frame_callback(Some(focus_frame), std::ptr::null_mut()) };
    }

    /// Moves the mouse to (`x`, `y`) in the window, then presses (`Some(true)`) or releases
    /// (`Some(false)`) the left button, as if the OS had sent it. For tests.
    pub fn inject_mouse(&self, x: f32, y: f32, button_down: Option<bool>) {
        // SAFETY: plain calls.
        unsafe {
            raven::Rwindow_inject_mouse_move(self.0, x, y);
            if let Some(down) = button_down {
                raven::Rwindow_inject_mouse_button(self.0, R_MOUSE_LEFT, c_int::from(down));
            }
        }
    }

    /// Turns the mouse wheel by `wy` lines at (`x`, `y`) in the window, with Ctrl held when
    /// `ctrl`. For tests.
    pub fn inject_wheel(&self, x: f32, y: f32, wy: f32, ctrl: bool) {
        // Injected input is handled at once, so the wheel event sees the Ctrl state.
        // SAFETY: plain calls.
        unsafe {
            raven::Rwindow_inject_mouse_move(self.0, x, y);
            if ctrl {
                raven::Rwindow_inject_key(self.0, R_KEY_LEFT_CTRL, 1, R_MOD_CTRL as u32);
            }
            raven::Rwindow_inject_mouse_wheel(self.0, 0.0, wy);
            if ctrl {
                raven::Rwindow_inject_key(self.0, R_KEY_LEFT_CTRL, 0, 0);
            }
        }
    }
}

unsafe extern "C" fn close_trampoline(_window: Rid, user: *mut c_void) -> c_int {
    // SAFETY: Raven passes back the `CloseFn` box given in `on_close`, on the UI thread.
    // A nested call keeps the window open.
    let close = unsafe { with_callback::<CloseFn, _>(user, |f| f()) }.unwrap_or(false);
    c_int::from(close)
}

struct FocusWatch {
    window: Rid,
    was_active: Cell<bool>,
    callback: RefCell<Box<dyn FnMut()>>,
}

thread_local! {
    /// The `on_focus_gained` callbacks. Only the UI thread's list is ever used.
    static FOCUS_WATCHES: RefCell<Vec<Rc<FocusWatch>>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "C" fn focus_frame(_delta_seconds: f64, _user: *mut c_void) {
    // Work on a copy of the list: a callback may add watches, or run a nested frame loop that
    // comes back here.
    let watches = FOCUS_WATCHES.with_borrow_mut(|watches| {
        watches.retain(|watch| is_valid(watch.window));
        watches.clone()
    });
    for watch in watches {
        // (an earlier callback may have closed this window)
        // SAFETY: plain call.
        let active =
            is_valid(watch.window) && unsafe { raven::Rwindow_is_active(watch.window) != 0 };
        let gained = active && !watch.was_active.replace(active);
        if gained && let Ok(mut callback) = watch.callback.try_borrow_mut() {
            callback();
        }
    }
}

fn is_valid(window: Rid) -> bool {
    // SAFETY: plain call.
    unsafe { raven::Rwindow_is_valid(window) != 0 }
}

/// Drops the `on_focus_gained` callbacks after a shutdown: their windows are gone, and a new
/// init may hand out the same handles again.
pub(crate) fn forget_focus_watches() {
    FOCUS_WATCHES.with_borrow_mut(Vec::clear);
}
