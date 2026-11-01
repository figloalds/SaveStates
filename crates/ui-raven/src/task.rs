//! Work handed to the UI thread, to Raven's workers, or to later.

use crate::{leak, reclaim, with_callback};
use raven::{R_INVALID_ID, Rid};
use std::ffi::{c_int, c_void};

type TaskFn = Box<dyn FnOnce() + Send>;
type TimerFn = Box<dyn FnMut()>;

/// Runs `f` once on the UI thread, during the next frame, and wakes the frame loop. Any thread:
/// this is how worker threads hand results to the UI.
pub fn post(f: impl FnOnce() + Send + 'static) {
    // SAFETY: the trampoline takes the box back and runs it once.
    unsafe { raven::Rtask_post(Some(task_trampoline), into_user(f)) };
}

/// Runs `f` once on a worker thread of Raven's job system. Any thread.
pub fn run_async(f: impl FnOnce() + Send + 'static) {
    // SAFETY: the trampoline takes the box back and runs it once.
    unsafe { raven::Rtask_run_async(Some(task_trampoline), into_user(f)) };
}

fn into_user(f: impl FnOnce() + Send + 'static) -> *mut c_void {
    let task: TaskFn = Box::new(f);
    Box::into_raw(Box::new(task)).cast()
}

unsafe extern "C" fn task_trampoline(user: *mut c_void) {
    // SAFETY: `user` comes from `into_user`, and Raven runs each task once.
    let task = unsafe { Box::from_raw(user.cast::<TaskFn>()) };
    task();
}

/// Calls `f` on the UI thread after `seconds`, then every `seconds` when `repeat`.
pub fn timer(seconds: f64, repeat: bool, f: impl FnMut() + 'static) -> Timer {
    let Some(user) = leak::<TimerFn>(Box::new(f)) else {
        return Timer(R_INVALID_ID);
    };
    let trampoline = if repeat {
        repeat_trampoline
    } else {
        once_trampoline
    };
    // SAFETY: `user` is a `TimerFn` box; a repeating timer never frees it, a one-shot timer
    // takes it back when it fires.
    let id = unsafe { raven::Rtimer_create(seconds, c_int::from(repeat), Some(trampoline), user) };
    if id == R_INVALID_ID {
        // Refused (a negative delay): Raven kept nothing.
        // SAFETY: Raven never saw this box.
        drop(unsafe { reclaim::<TimerFn>(user) });
    }
    Timer(id)
}

unsafe extern "C" fn repeat_trampoline(_timer: Rid, user: *mut c_void) {
    // SAFETY: Raven passes back the `TimerFn` box given in `timer`, on the UI thread.
    unsafe { with_callback::<TimerFn, _>(user, |f| f()) };
}

unsafe extern "C" fn once_trampoline(_timer: Rid, user: *mut c_void) {
    // SAFETY: a one-shot timer is deleted before Raven calls it, so this is its only call.
    let mut f = unsafe { reclaim::<TimerFn>(user) };
    f();
}

/// A running timer. Dropping it leaves the timer running.
#[derive(Debug)]
pub struct Timer(Rid);

impl Timer {
    /// Stops the timer. Nothing happens if it already fired (one-shot) or was refused.
    pub fn cancel(self) {
        // SAFETY: plain calls; the handle is generational, so a fired timer stays invalid.
        unsafe {
            if raven::Rtimer_is_valid(self.0) != 0 {
                raven::Rtimer_destroy(self.0);
            }
        }
    }
}
