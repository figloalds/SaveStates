//! Safe Raven wrapper: the only UI API the rest of SaveStates uses.
//!
//! Raven is a C GUI library (vendored binding `raven-gui`, imported as `raven`). This crate is
//! the only one that depends on it or contains `unsafe`.
//!
//! Threading: Raven's UI thread is the thread that called [`init`]. Every function here must be
//! called on it, except [`task::post`], [`task::run_async`], [`quit`], [`is_ui_thread`] and
//! [`native_error_box`]. Elsewhere the call does nothing, and a closure given to it is dropped
//! without being registered. Almost every Raven function checks its thread itself and records
//! an error. Two do not: `Rtimer_is_valid` and `Rwindow_is_valid`, which read Raven's pools
//! directly. The wrapper calls them only on the UI thread ([`Timer::cancel`] checks first, and
//! [`Timer`] is neither `Send` nor `Sync`; the window check runs only in a frame callback).
//! Raven calls every callback on the UI thread.
//!
//! Callbacks: closures are boxed and handed to Raven as `user_data` through `extern "C"`
//! trampolines, and are never freed (the app creates each window, dialog and widget once and
//! reuses it). A frame loop that runs inside a callback (a waiting dialog such as
//! [`dialog::message`]) can make Raven call the same callback again before it returns: that
//! nested call is skipped instead of entering the closure twice. A closure meant to run once
//! (a dialog's `on_closed`, a one-shot timer) leaves its slot before it runs, so it runs at
//! most once whatever Raven does.

pub mod dialog;
mod event;
mod lists;
pub mod locale;
mod native;
mod painter;
mod runtime;
pub mod task;
mod widget;
mod window;

pub use event::{Event, Key, MouseButton};
pub use lists::{combo, list, tabs, tree};
pub use locale::{tr, tr_fmt, tr_plural};
pub use native::{native_error_box, theme};
pub use painter::{Painter, Rgba, custom, text_width};
pub use runtime::{Backend, draw_frame, init, is_ui_thread, open_url, quit, shutdown, wait_frame};
pub use task::Timer;
pub use tree::TreeItem;
pub use widget::{Widget, bind_action, widget_from_xml};
pub use window::Window;

use std::cell::RefCell;
use std::ffi::{CString, c_char, c_void};
use std::ptr;

// Raven's static library calls WinHTTP (Markdown images), which the vendored binding's build
// script does not link. Declaring it here makes every binary that depends on this crate link it.
#[cfg(windows)]
#[link(name = "winhttp")]
unsafe extern "C" {}

/// A text for Raven. A NUL inside the text ends it.
fn cstr(text: &str) -> CString {
    raven::cstr(text)
}

/// A text Raven returned, copied ("" for a null pointer).
///
/// # Safety
/// `pointer` is null or a NUL-terminated text that is still valid.
unsafe fn text(pointer: *const c_char) -> String {
    // SAFETY: forwarded from the caller.
    unsafe { raven::text(pointer) }
}

/// Forgets the error Raven recorded last, so that a failure that follows reports its own.
fn clear_last_error() {
    // SAFETY: both outputs may be null.
    unsafe { raven::Rerror_get_last(ptr::null_mut(), ptr::null_mut()) };
}

/// The error Raven recorded last, as "detail (R_ERROR_...)", and forgets it.
fn last_error() -> String {
    let mut detail: *const c_char = ptr::null();
    // SAFETY: `detail` is a valid output; Raven points it at its own buffer, which stays valid
    // until the next call, and the texts of `Rerror_to_string` are static.
    let (code, detail, name) = unsafe {
        let code = raven::Rerror_get_last(&mut detail, ptr::null_mut());
        (code, text(detail), text(raven::Rerror_to_string(code)))
    };
    if code == raven::R_NO_ERROR {
        "Raven failed without recording an error".to_owned()
    } else if detail.is_empty() {
        name
    } else {
        format!("{detail} ({name})")
    }
}

/// Converts a count or an index for Raven, saturating at `i32::MAX`.
fn to_int(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// Converts an index Raven returned: negative means none.
fn index(value: i32) -> Option<usize> {
    usize::try_from(value).ok()
}

/// Boxes a callback for Raven's `user_data`. It is never freed, unless a trampoline takes it
/// back with [`reclaim`].
///
/// `None` off the UI thread: Raven refuses to register callbacks there, and would call them on
/// another thread than the one that made them.
fn leak<T: 'static>(callback: T) -> Option<*mut c_void> {
    if !is_ui_thread() {
        return None;
    }
    Some(Box::into_raw(Box::new(RefCell::new(callback))).cast())
}

/// Runs `call` with the callback behind `user_data`, unless that callback is already running
/// (a nested frame loop re-entered it): then it returns `None`.
///
/// # Safety
/// `user_data` comes from [`leak`] with the same `T` and was not reclaimed, and this runs on
/// the UI thread.
unsafe fn with_callback<T, R>(user_data: *mut c_void, call: impl FnOnce(&mut T) -> R) -> Option<R> {
    // SAFETY: forwarded from the caller; the box is never freed while Raven may call it.
    let cell = unsafe { &*user_data.cast::<RefCell<T>>() };
    let mut callback = cell.try_borrow_mut().ok()?;
    Some(call(&mut callback))
}

/// Takes back a callback boxed by [`leak`] that Raven will not call again.
///
/// # Safety
/// `user_data` comes from [`leak`] with the same `T`, was not reclaimed before, and Raven will
/// never pass it to a trampoline again.
unsafe fn reclaim<T>(user_data: *mut c_void) -> T {
    // SAFETY: forwarded from the caller.
    unsafe { Box::from_raw(user_data.cast::<RefCell<T>>()) }.into_inner()
}
