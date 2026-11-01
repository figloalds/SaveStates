//! The theme, and the one OS call that works without Raven.

/// The look of every window.
pub mod theme {
    use crate::{clear_last_error, last_error};
    use std::ffi::c_int;

    /// Loads a theme file's text (`<Theme Name="..." ...>`) and makes it the theme in use.
    /// The error is Raven's text.
    pub fn load_xml(xml: &str) -> Result<(), String> {
        if xml.is_empty() {
            return Err("the theme XML is empty".to_owned());
        }
        clear_last_error();
        // SAFETY: the text and its (non-zero) length describe a live buffer. Raven returns the
        // theme's name, valid until shutdown, or null after recording an error.
        let name = unsafe { raven::Rtheme_load_from_memory(xml.as_ptr().cast(), xml.len()) };
        // SAFETY: `name` is valid; `Rtheme_use` records an error when it returns false.
        if name.is_null() || unsafe { raven::Rtheme_use(name) } == 0 {
            return Err(last_error());
        }
        Ok(())
    }

    /// Tells whether the theme is a dark one (the tint of glass, the colors of code).
    pub fn set_dark(on: bool) {
        // SAFETY: plain call.
        unsafe { raven::Rtheme_set_dark(c_int::from(on)) };
    }

    /// Sets the accent color and the shades that follow from it (hover, focus, selection).
    pub fn set_accent(r: f32, g: f32, b: f32) {
        // SAFETY: plain call.
        unsafe { raven::Rtheme_set_accent(r, g, b) };
    }
}

/// Shows an error in the OS's own message box and waits for the user to close it: for failures
/// before Raven runs (it cannot start). Off Windows, prints to stderr instead.
pub fn native_error_box(title: &str, body: &str) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (title, body) = (wide(title), wide(body));
        // SAFETY: both texts are NUL-terminated UTF-16 that outlive the call; no owner window.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                body.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            )
        };
    }
    #[cfg(not(windows))]
    eprintln!("{title}: {body}");
}
