//! Starting and stopping Raven, and the frame loop.

use crate::{clear_last_error, cstr, last_error};
use raven::{R_BACKEND_NULL, R_BACKEND_VULKAN, R_TRUE, RInitDesc};

/// The renderer Raven starts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// The real renderer, in OS windows.
    Vulkan,
    /// Renders nothing and opens no OS window: for tests. Input injected into windows still
    /// reaches the widgets.
    Null,
}

/// Starts Raven. The calling thread becomes the UI thread. The error is Raven's own text.
pub fn init(b: Backend) -> Result<(), String> {
    clear_last_error();
    let ok = match b {
        // SAFETY: plain call.
        Backend::Vulkan => unsafe { raven::Rinit(R_BACKEND_VULKAN) },
        Backend::Null => {
            // Raven's own tests pair the NULL backend with the headless (null) platform.
            // A zeroed descriptor selects the default for every other field.
            // SAFETY: `RInitDesc` is plain data for which all zeros is valid.
            let mut desc: RInitDesc = unsafe { std::mem::zeroed() };
            desc.backend = R_BACKEND_NULL;
            desc.headless = R_TRUE;
            // SAFETY: `desc` outlives the call.
            unsafe { raven::Rinit_ex(&desc) }
        }
    };
    if ok != 0 { Ok(()) } else { Err(last_error()) }
}

/// Destroys every window, widget and resource and stops Raven's threads.
pub fn shutdown() {
    // SAFETY: plain call; Raven refuses it off the UI thread.
    unsafe { raven::Rshutdown() };
    crate::window::forget_focus_watches();
}

/// Sleeps until there is something to do (at most `timeout_s` seconds; negative waits without
/// limit), then produces a frame. False once [`quit`] was called or the last window closed.
pub fn wait_frame(timeout_s: f64) -> bool {
    // SAFETY: plain call.
    unsafe { raven::Rwait_frame(timeout_s) != 0 }
}

/// Produces a frame without waiting: OS events, posted tasks, timers, then layout and paint of
/// what changed. False once [`quit`] was called or the last window closed.
pub fn draw_frame() -> bool {
    // SAFETY: plain call.
    unsafe { raven::Rdraw_frame() != 0 }
}

/// Makes the frame loop end with `code`. Any thread.
pub fn quit(code: i32) {
    // SAFETY: plain call; thread-safe in Raven.
    unsafe { raven::Rquit(code) };
}

/// True on Raven's UI thread (the thread that called [`init`]). Any thread.
pub fn is_ui_thread() -> bool {
    // SAFETY: plain call; thread-safe in Raven.
    unsafe { raven::Ris_ui_thread() != 0 }
}

/// Opens an address with the program the user has for it (a web page in the browser).
/// False when the OS could not be asked.
pub fn open_url(url: &str) -> bool {
    let url = cstr(url);
    // SAFETY: `url` outlives the call.
    unsafe { raven::Ropen_url(url.as_ptr()) != 0 }
}
