//! Dialogs inside a window, message boxes and the folder picker.

use crate::widget::Widget;
use crate::window::Window;
use crate::{cstr, leak, reclaim, text};
use raven::*;
use std::ffi::{c_int, c_void};
use std::path::PathBuf;

type ClosedFn = Box<dyn FnOnce(i32)>;

/// Shows `content` as a dialog of `win`. The window takes `content` and deletes it once the
/// dialog closes, so read what it holds before, or in `on_closed`, which gets the result given
/// to [`close`] (0 when dismissed with Escape).
pub fn open(win: Window, content: Widget, on_closed: impl FnOnce(i32) + 'static) {
    let Some(user) = leak::<ClosedFn>(Box::new(on_closed)) else {
        return;
    };
    // SAFETY: `user` is a `ClosedFn` box; Raven calls the closed callback at most once, and the
    // trampoline takes the box back then.
    unsafe { raven::Rdialog_open(win.0, content.0, Some(closed_trampoline), user) };
}

unsafe extern "C" fn closed_trampoline(_content: Rid, result: c_int, user: *mut c_void) {
    // SAFETY: `user` is the box given in `open`; Raven drops the dialog before calling, so this
    // is the only call.
    let on_closed = unsafe { reclaim::<ClosedFn>(user) };
    on_closed(result);
}

/// Closes the dialog that contains `content` (the content itself or anything inside it).
pub fn close(content: Widget, result: i32) {
    // SAFETY: plain call.
    unsafe { raven::Rdialog_close(content.0, result) };
}

/// The buttons of a message box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Buttons {
    Ok,
    OkCancel,
    YesNo,
    YesNoCancel,
}

/// What the user answered. A message box that was dismissed (Escape) answers `Cancel`, or
/// `No` when it has no Cancel button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Ok,
    Cancel,
    Yes,
    No,
}

/// Shows a message in `win` (`None`: the app's first window) and waits for the answer, running
/// frames meanwhile.
pub fn message(win: Option<Window>, title: &str, text: &str, b: Buttons) -> Answer {
    let buttons = match b {
        Buttons::Ok => R_MESSAGE_OK,
        Buttons::OkCancel => R_MESSAGE_OK_CANCEL,
        Buttons::YesNo => R_MESSAGE_YES_NO,
        Buttons::YesNoCancel => R_MESSAGE_YES_NO_CANCEL,
    };
    let (title, text) = (cstr(title), cstr(text));
    let window = win.map_or(R_INVALID_ID, |w| w.0);
    // SAFETY: both texts outlive the call.
    match unsafe { raven::Rmessage_box(window, title.as_ptr(), text.as_ptr(), buttons) } {
        R_MESSAGE_RESULT_OK => Answer::Ok,
        R_MESSAGE_RESULT_YES => Answer::Yes,
        R_MESSAGE_RESULT_NO => Answer::No,
        _ => Answer::Cancel,
    }
}

/// Asks for a folder and waits for the answer. `None` when the user cancelled.
pub fn select_folder(title: &str) -> Option<PathBuf> {
    let title = cstr(title);
    // SAFETY: `RFileDialogDesc` is plain data for which all zeros is valid ("every field may be
    // left to zero").
    let mut desc: RFileDialogDesc = unsafe { std::mem::zeroed() };
    desc.parent = R_INVALID_ID;
    desc.title = title.as_ptr();
    // SAFETY: `desc` and `title` outlive the call; the path is valid until the next file dialog,
    // and is copied at once.
    let path = unsafe { text(raven::Rfile_dialog_select_folder(&desc)) };
    (!path.is_empty()).then(|| PathBuf::from(path))
}
