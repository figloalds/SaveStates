//! Work handed to the UI thread, to Raven's workers, or to later.

use crate::{is_ui_thread, leak, reclaim, with_callback};
use raven::{R_INVALID_ID, Rid};
use std::ffi::{c_int, c_void};
use std::marker::PhantomData;

type TaskFn = Box<dyn FnOnce() + Send>;
/// `None` once a one-shot timer has run, so a second call from Raven does nothing.
type TimerSlot = Option<Box<dyn FnMut()>>;

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
    let Some(user) = leak::<TimerSlot>(Some(Box::new(f))) else {
        return Timer::new(R_INVALID_ID);
    };
    let trampoline = if repeat {
        repeat_trampoline
    } else {
        once_trampoline
    };
    // SAFETY: `user` is a `TimerSlot` box that is never freed while Raven holds it.
    let id = unsafe { raven::Rtimer_create(seconds, c_int::from(repeat), Some(trampoline), user) };
    if id == R_INVALID_ID {
        // Refused (a negative delay): Raven kept nothing.
        // SAFETY: Raven never saw this box.
        drop(unsafe { reclaim::<TimerSlot>(user) });
    }
    Timer::new(id)
}

unsafe extern "C" fn repeat_trampoline(_timer: Rid, user: *mut c_void) {
    // SAFETY: Raven passes back the `TimerSlot` box given in `timer`, on the UI thread.
    unsafe {
        with_callback::<TimerSlot, _>(user, |slot| {
            if let Some(f) = slot {
                f();
            }
        })
    };
}

unsafe extern "C" fn once_trampoline(_timer: Rid, user: *mut c_void) {
    // The closure leaves its slot before it runs: whatever Raven does, it runs at most once.
    // SAFETY: Raven passes back the `TimerSlot` box given in `timer`, on the UI thread.
    let f = unsafe { with_callback::<TimerSlot, _>(user, Option::take) }.flatten();
    if let Some(mut f) = f {
        f();
    }
}

/// A running timer. Dropping it leaves the timer running.
///
/// Neither `Send` nor `Sync`: `Rtimer_is_valid` reads Raven's timer pool without a thread
/// check, so a handle must not reach a thread where it could race the UI thread.
#[derive(Debug)]
pub struct Timer {
    id: Rid,
    _ui_thread: PhantomData<*const ()>,
}

// Fails to compile if `Timer` becomes `Send` or `Sync` (the `static_assertions` trick: when the
// bound holds, both impls apply and `some_item` is ambiguous).
const _: fn() = || {
    trait AmbiguousIfSend<A> {
        fn some_item() {}
    }
    impl<T: ?Sized> AmbiguousIfSend<()> for T {}
    impl<T: ?Sized + Send> AmbiguousIfSend<u8> for T {}
    let _ = <Timer as AmbiguousIfSend<_>>::some_item;

    trait AmbiguousIfSync<A> {
        fn some_item() {}
    }
    impl<T: ?Sized> AmbiguousIfSync<()> for T {}
    impl<T: ?Sized + Sync> AmbiguousIfSync<u8> for T {}
    let _ = <Timer as AmbiguousIfSync<_>>::some_item;
};

impl Timer {
    fn new(id: Rid) -> Timer {
        Timer {
            id,
            _ui_thread: PhantomData,
        }
    }

    /// Stops the timer. Nothing happens if it already fired (one-shot), was refused, or if this
    /// is not the UI thread.
    pub fn cancel(self) {
        if !is_ui_thread() {
            return;
        }
        // SAFETY: plain calls, on the UI thread; the handle is generational, so a fired timer
        // stays invalid.
        unsafe {
            if raven::Rtimer_is_valid(self.id) != 0 {
                raven::Rtimer_destroy(self.id);
            }
        }
    }
}
