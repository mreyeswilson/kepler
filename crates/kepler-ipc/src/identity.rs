//! OS-derived user identity. Never read from the environment: variables such as
//! `USERNAME`/`USER` are inheritable and spoofable (a terminal launched by another
//! service can carry `USERNAME=SYSTEM`), which used to split one user across two daemons.
//! This is a thin FFI layer over documented calls; no extra dependency is added.

#![allow(unsafe_code)]

/// Raw OS user name (Windows) or `uid<N>` (Unix). `None` if the OS call fails.
pub fn os_user_raw() -> Option<String> {
    #[cfg(windows)]
    {
        #[link(name = "advapi32")]
        extern "system" {
            fn GetUserNameW(buf: *mut u16, len: *mut u32) -> i32;
        }
        let mut buf = [0u16; 257]; // UNLEN (256) + NUL
        let mut len = buf.len() as u32;
        // SAFETY: `buf` is valid for `len` u16 units; on success `len` includes the NUL.
        let ok = unsafe { GetUserNameW(buf.as_mut_ptr(), &mut len) };
        if ok == 0 || len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(
            &buf[..(len as usize - 1).min(buf.len())],
        ))
    }
    #[cfg(unix)]
    {
        extern "C" {
            fn getuid() -> u32;
        }
        // SAFETY: getuid(2) takes no arguments, cannot fail and has no preconditions.
        Some(format!("uid{}", unsafe { getuid() }))
    }
    #[cfg(not(any(windows, unix)))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn identity_comes_from_the_os() {
        assert!(super::os_user_raw().is_some_and(|s| !s.is_empty()));
    }
}
