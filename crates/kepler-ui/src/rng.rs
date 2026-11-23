//! Session token from the operating system's CSPRNG (no new dependency).
//!
//! Windows: `BCryptGenRandom` with the system-preferred RNG. Unix: `/dev/urandom`. A failure to
//! obtain randomness is an error: the server refuses to start rather than use a guessable token.

#![allow(unsafe_code)]

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;

    #[link(name = "bcrypt")]
    extern "system" {
        fn BCryptGenRandom(algorithm: *mut c_void, buffer: *mut u8, len: u32, flags: u32) -> i32;
    }

    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;

    pub fn fill(buf: &mut [u8]) -> bool {
        let Ok(len) = u32::try_from(buf.len()) else {
            return false;
        };
        // SAFETY: `buf` is a valid writable region of `len` bytes; a null algorithm handle with
        // BCRYPT_USE_SYSTEM_PREFERRED_RNG is the documented way to request system randomness.
        unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                buf.as_mut_ptr(),
                len,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            ) == 0
        }
    }
}

#[cfg(not(windows))]
mod sys {
    use std::io::Read;

    pub fn fill(buf: &mut [u8]) -> bool {
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(buf))
            .is_ok()
    }
}

/// 256 random bits as 64 lowercase hex characters.
pub fn new_token() -> Result<String, String> {
    let mut b = [0u8; 32];
    if !sys::fill(&mut b) {
        return Err("operating system randomness is unavailable".into());
    }
    if b.iter().all(|x| *x == 0) {
        return Err("operating system randomness returned zeros".into());
    }
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_hex_and_distinct() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
