//! The app's log: `log.txt` next to the settings (`%APPDATA%\SaveStates\log.txt`).
//!
//! A release build has no console, so what went wrong goes to this file, where someone helping
//! the user can read it: the app's diagnostics (the [`log!`] lines) and every panic, with where
//! it happened. Writing it never stops the app: a line that cannot be written is dropped. The
//! file stays small: once it grows past 2 MB, only its last megabyte is kept.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError, TryLockError};

/// Past this size the log is cut down...
const MAX_BYTES: u64 = 2 * 1024 * 1024;
/// ...to about its last megabyte, from the start of a line.
const KEEP_BYTES: usize = 1024 * 1024;

/// The log file, once [`install`] ran. Before that (and in tests) lines go to stderr only.
static FILE: OnceLock<PathBuf> = OnceLock::new();
/// One writer at a time: the UI thread, the project workers and the AI thread all log.
static WRITING: Mutex<()> = Mutex::new(());

/// Writes a line to the log, formatted like `format!`.
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::log::write(&format!($($arg)*))
    };
}

/// The log file next to the settings file `settings`.
pub fn next_to(settings: &Path) -> PathBuf {
    settings.with_file_name("log.txt")
}

/// Logs to `file` from now on, and writes every panic there as well.
pub fn install(file: PathBuf) {
    let file = FILE.get_or_init(|| file).clone();
    let previous = std::panic::take_hook();
    std::panic::set_hook(panic_hook(file, previous));
}

/// Writes `message` to the log (and to stderr, which only a debug build shows).
pub fn write(message: &str) {
    eprintln!("{message}");
    if let Some(file) = FILE.get() {
        let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
        append(file, &line(chrono::Local::now().naive_local(), message));
    }
}

type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static>;

/// A panic hook that writes the panic to `file`, then runs `then` (Rust's own hook, which
/// prints it to stderr).
fn panic_hook(file: PathBuf, then: Hook) -> Hook {
    Box::new(move |info| {
        let text = line(chrono::Local::now().naive_local(), &panic_text(info));
        // Never wait here: the panic may have happened while this thread was writing.
        match WRITING.try_lock() {
            Ok(_writing) => append(&file, &text),
            Err(TryLockError::Poisoned(writing)) => {
                let _writing = writing.into_inner();
                append(&file, &text);
            }
            Err(TryLockError::WouldBlock) => append(&file, &text),
        }
        then(info);
    })
}

/// What the log says about a panic: its message, and the file, line and thread it happened in.
fn panic_text(info: &PanicHookInfo<'_>) -> String {
    let message = info.payload_as_str().unwrap_or("(no message)");
    let place = info.location().map_or_else(
        || "an unknown place".to_owned(),
        |at| format!("{}:{}:{}", at.file(), at.line(), at.column()),
    );
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("unnamed");
    format!("PANIC in thread {thread} at {place}: {message}")
}

/// One line of the log: the local time, then `message` (without the line breaks it may end
/// with, as git's error output does).
fn line(now: chrono::NaiveDateTime, message: &str) -> String {
    let message = message.trim_end();
    format!("{} {message}\n", now.format("%Y-%m-%d %H:%M:%S"))
}

/// Adds `text` at the end of `file`, making its folder if needed, then keeps the file small.
/// Gives up quietly when the file cannot be written.
fn append(file: &Path, text: &str) {
    if let Some(folder) = file.parent() {
        let _ = fs::create_dir_all(folder);
    }
    let Ok(mut out) = OpenOptions::new().create(true).append(true).open(file) else {
        return;
    };
    if out.write_all(text.as_bytes()).is_err() {
        return;
    }
    let size = out.metadata().map_or(0, |meta| meta.len());
    drop(out);
    if size > MAX_BYTES {
        keep_the_end(file);
    }
}

/// Cuts `file` down to its last [`KEEP_BYTES`], from the first line that starts in them.
fn keep_the_end(file: &Path) {
    let Ok(bytes) = fs::read(file) else { return };
    let tail = &bytes[bytes.len().saturating_sub(KEEP_BYTES)..];
    let start = tail.iter().position(|&b| b == b'\n').map_or(0, |i| i + 1);
    let _ = fs::write(file, &tail[start..]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn noon() -> chrono::NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, 11, 2)
            .and_then(|day| day.and_hms_opt(12, 30, 5))
            .unwrap()
    }

    #[test]
    fn lines_are_added_with_the_time_and_the_folder_is_made() {
        let dir = tempfile::tempdir().unwrap();
        let file = next_to(&dir.path().join("SaveStates").join("settings.toml"));
        assert_eq!(file, dir.path().join("SaveStates").join("log.txt"));

        append(&file, &line(noon(), "the theme could not be loaded"));
        // Git's error output ends with a line break: no empty line follows.
        append(&file, &line(noon(), "a Quick Save failed: fatal: bad\n"));

        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "2026-11-02 12:30:05 the theme could not be loaded\n\
             2026-11-02 12:30:05 a Quick Save failed: fatal: bad\n"
        );
    }

    #[test]
    fn a_log_past_its_cap_keeps_its_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("log.txt");
        let old: String = (0..60_000)
            .map(|i| format!("line {i:06} {}\n", "x".repeat(30)))
            .collect();
        assert!(old.len() as u64 > MAX_BYTES);
        fs::write(&file, &old).unwrap();

        append(&file, "the newest line\n");

        let kept = fs::read_to_string(&file).unwrap();
        assert!(kept.len() <= KEEP_BYTES, "{} bytes kept", kept.len());
        assert!(kept.len() > KEEP_BYTES / 2, "{} bytes kept", kept.len());
        assert!(kept.ends_with("line 059999 xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\nthe newest line\n"));
        assert!(
            kept.starts_with("line "),
            "starts mid-line: {:?}",
            &kept[..40]
        );
    }

    #[test]
    fn a_log_that_cannot_be_written_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        // Its folder is a file.
        let blocker = dir.path().join("SaveStates");
        fs::write(&blocker, "not a folder").unwrap();
        append(&blocker.join("log.txt"), "lost\n");
        // It is a folder.
        append(dir.path(), "lost\n");
        assert_eq!(fs::read_to_string(&blocker).unwrap(), "not a folder");
    }

    #[test]
    fn a_panic_is_written_with_where_it_happened() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("log.txt");
        // Panic hooks are for the whole test process: Rust's own hook keeps running after this
        // one, and comes back afterwards.
        let rust_hook = Arc::new(std::panic::take_hook());
        let then = rust_hook.clone();
        std::panic::set_hook(panic_hook(file.clone(), Box::new(move |info| then(info))));
        let crashed = std::thread::Builder::new()
            .name("worker".to_owned())
            .spawn(|| panic!("boom {}", 42))
            .unwrap()
            .join();
        std::panic::set_hook(Box::new(move |info| rust_hook(info)));

        assert!(crashed.is_err());
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("PANIC in thread worker at "), "{text}");
        assert!(text.contains("log.rs:"), "{text}");
        assert!(text.trim_end().ends_with(": boom 42"), "{text}");
    }
}
