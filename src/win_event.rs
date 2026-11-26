//! An owned Win32 auto-reset event, shared through `Arc`, used to wake a
//! background thread blocked in a Windows wait.
//!
//! Ownership is the whole point of this type: the kernel handle is closed in
//! exactly one place (`Drop`, when the last `Arc` clone goes away), so no
//! caller has to reason about who closes it or whether a copy might signal a
//! handle value that has already been closed and reused.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};

pub struct Event(HANDLE);

// SAFETY: an event is a kernel object with no thread affinity. Any thread may
// signal or wait on it, and the kernel serializes those operations. The handle
// is only closed in `Drop`, which runs once no clone can use it any more.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl Event {
    /// Creates an anonymous, auto-reset, initially clear event. Panics when
    /// Windows cannot create it: a kernel object allocation failure at startup
    /// is not recoverable here.
    pub fn new() -> Self {
        let handle = unsafe {
            // SAFETY: null attributes and a null name create a plain,
            // non-inheritable, anonymous event.
            CreateEventW(None, false, false, PCWSTR::null())
        }
        .unwrap_or_else(|error| panic!("failed to create a wake event: {error:?}"));
        Event(handle)
    }

    /// Wakes the thread waiting on this event.
    pub fn signal(&self) {
        let _ = unsafe {
            // SAFETY: the handle is live for as long as `self` is.
            SetEvent(self.0)
        };
    }

    /// The raw handle, for passing to a Windows wait function. It is only
    /// valid while `self` is alive.
    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Default for Event {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        let _ = unsafe {
            // SAFETY: this is the only close site, and no clone can reach the
            // handle once the last owner is dropped.
            CloseHandle(self.0)
        };
    }
}
