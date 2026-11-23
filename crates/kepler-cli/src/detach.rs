//! Windows handle hygiene for lazily started daemons.
//!
//! `CreateProcess` is called with `bInheritHandles = TRUE`, so a detached daemon
//! would inherit the *caller's* stdout/stderr pipe even when it is given NUL
//! stdio. A harness reading `kepler emit` output until EOF would then block for
//! the whole life of the daemon. Marking our own standard handles
//! non-inheritable before spawning prevents that.

#![allow(unsafe_code)]

use std::ffi::c_void;

type Handle = *mut c_void;

#[link(name = "kernel32")]
extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn SetHandleInformation(h: Handle, mask: u32, flags: u32) -> i32;
}

const STD_INPUT_HANDLE: u32 = 0xFFFF_FFF6; // (DWORD)-10
const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5; // (DWORD)-11
const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4; // (DWORD)-12
const HANDLE_FLAG_INHERIT: u32 = 1;

/// Make this process's stdin/stdout/stderr non-inheritable (best effort).
pub fn make_std_handles_non_inheritable() {
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: GetStdHandle returns this process's own handle (or NULL/INVALID);
        // SetHandleInformation only clears the inherit flag, it does not close or move it.
        unsafe {
            let h = GetStdHandle(which);
            if !h.is_null() && h as isize != -1 {
                SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}
