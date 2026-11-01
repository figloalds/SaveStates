//! Translations: Raven's CSV tables (`key,pt-BR,en`), plurals and `{0}` arguments.

use crate::{clear_last_error, cstr, last_error, leak, text, with_callback};
use std::ffi::{c_char, c_void};

type ChangedFn = Box<dyn FnMut()>;

/// Loads a translation table and returns the number of keys read. The first column after
/// `key` is the fallback language. The error is Raven's text.
pub fn load_csv(csv: &str) -> Result<usize, String> {
    // A Windows checkout may turn the line ends of the embedded table into CRLF. Raven trims a
    // "\r" at the end of a plain field, but keeps the "\r\n" inside a quoted multi-line text.
    let csv = csv.replace("\r\n", "\n");
    if csv.is_empty() {
        return Err("the translation table is empty".to_owned());
    }
    clear_last_error();
    // SAFETY: the text and its length describe a live buffer.
    let keys = unsafe { raven::Rlocale_load_csv_from_memory(csv.as_ptr().cast(), csv.len()) };
    usize::try_from(keys).map_err(|_| last_error())
}

/// Chooses the language by its column code ("pt-BR", "en"). Widgets that show a key follow.
pub fn set_language(code: &str) {
    let code = cstr(code);
    // SAFETY: `code` outlives the call. A code with no column falls back, which is what we want.
    unsafe { raven::Rlocale_set_language(code.as_ptr()) };
}

/// The language whose texts are shown when the current one has none.
pub fn set_fallback(code: &str) {
    let code = cstr(code);
    // SAFETY: `code` outlives the call.
    unsafe { raven::Rlocale_set_fallback_language(code.as_ptr()) };
}

/// Called after the language or the tables changed (widgets that show keys are already
/// updated). Setting it again replaces the previous closure.
pub fn on_changed(f: impl FnMut() + 'static) {
    let Some(user) = leak::<ChangedFn>(Box::new(f)) else {
        return;
    };
    // SAFETY: `user` is a `ChangedFn` box that is never freed.
    unsafe { raven::Rlocale_set_changed_callback(Some(changed_trampoline), user) };
}

unsafe extern "C" fn changed_trampoline(_language: *const c_char, user: *mut c_void) {
    // SAFETY: Raven passes back the `ChangedFn` box given in `on_changed`, on the UI thread.
    unsafe { with_callback::<ChangedFn, _>(user, |f| f()) };
}

/// The text of `key` in the current language, else the fallback one, else the key itself.
pub fn tr(key: &str) -> String {
    let key = cstr(key);
    // SAFETY: `key` outlives the call; Raven returns a text it keeps (or `key`), copied at once.
    unsafe { text(raven::Rtr(key.as_ptr())) }
}

/// [`tr`] with `{0}`, `{1}`... replaced by `args`.
pub fn tr_fmt(key: &str, args: &[&str]) -> String {
    let key = cstr(key);
    let args: Vec<_> = args.iter().map(|a| cstr(a)).collect();
    let pointers: Vec<*const c_char> = args.iter().map(|a| a.as_ptr()).collect();
    // SAFETY: `key`, `args` and `pointers` outlive the call; the result is copied at once.
    unsafe {
        text(raven::Rtr_format(
            key.as_ptr(),
            pointers.as_ptr(),
            crate::to_int(pointers.len()),
        ))
    }
}

/// The plural form of `key` for `n`: `<key>.one` or `<key>.other` (by the rules of the current
/// language), with `{n}` replaced by `n`. Counts beyond the `i32` range are clamped to it.
pub fn tr_plural(key: &str, n: i64) -> String {
    let key = cstr(key);
    let n = i32::try_from(n).unwrap_or(if n < 0 { i32::MIN } else { i32::MAX });
    // SAFETY: `key` outlives the call; the result is copied at once.
    unsafe { text(raven::Rtr_plural(key.as_ptr(), n)) }
}
