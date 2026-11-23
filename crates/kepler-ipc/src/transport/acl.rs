//! Named pipe security: a protected DACL granting access only to the current
//! user's SID. This is the only `unsafe` code in the workspace; it is a thin
//! FFI layer over documented Win32 calls, with every pointer owned and freed here.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::io;
use std::ptr::null_mut;
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

type Handle = *mut c_void;

#[repr(C)]
struct SecurityAttributes {
    n_length: u32,
    lp_security_descriptor: *mut c_void,
    b_inherit_handle: i32,
}

#[repr(C)]
struct SidAndAttributes {
    sid: *mut c_void,
    attributes: u32,
}

#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        info: *mut c_void,
        len: u32,
        ret_len: *mut u32,
    ) -> i32;
    fn ConvertSidToStringSidW(sid: *mut c_void, out: *mut *mut u16) -> i32;
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        sddl: *const u16,
        revision: u32,
        sd: *mut *mut c_void,
        size: *mut u32,
    ) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn CloseHandle(h: Handle) -> i32;
    fn LocalFree(h: *mut c_void) -> *mut c_void;
}

const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_USER_CLASS: u32 = 1;
const SDDL_REVISION_1: u32 = 1;

/// SID of the current process token as a string (`S-1-5-21-...`).
pub fn current_user_sid_string() -> io::Result<String> {
    // SAFETY: standard token query. `token` is closed on every path; the buffer is
    // 8-byte aligned (Vec<u64>) and sized by the first call, so reading the
    // TOKEN_USER header from it is valid; the SID pointer lives inside `buf`.
    unsafe {
        let mut token: Handle = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let result = (|| {
            let mut len = 0u32;
            GetTokenInformation(token, TOKEN_USER_CLASS, null_mut(), 0, &mut len);
            if len == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            if GetTokenInformation(
                token,
                TOKEN_USER_CLASS,
                buf.as_mut_ptr().cast(),
                len,
                &mut len,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let user = &*(buf.as_ptr().cast::<SidAndAttributes>());
            let mut wide: *mut u16 = null_mut();
            if ConvertSidToStringSidW(user.sid, &mut wide) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut n = 0usize;
            while *wide.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(wide, n));
            LocalFree(wide.cast());
            Ok(s)
        })();
        CloseHandle(token);
        result
    }
}

/// SDDL: protected DACL (no inheritance) with one ACE: GENERIC_ALL for the user.
pub fn user_only_sddl() -> io::Result<String> {
    Ok(format!("D:P(A;;GA;;;{})", current_user_sid_string()?))
}

/// Create a pipe instance whose DACL only allows the current user.
pub fn create_user_only_pipe(opts: &ServerOptions, name: &str) -> io::Result<NamedPipeServer> {
    let sddl: Vec<u16> = user_only_sddl()?
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `sd` is allocated by the OS and freed with LocalFree after the pipe
    // is created (the kernel copies the descriptor). `attrs` outlives the call.
    unsafe {
        let mut sd: *mut c_void = null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut attrs = SecurityAttributes {
            n_length: std::mem::size_of::<SecurityAttributes>() as u32,
            lp_security_descriptor: sd,
            b_inherit_handle: 0,
        };
        let res = opts.create_with_security_attributes_raw(
            name,
            (&mut attrs as *mut SecurityAttributes).cast(),
        );
        LocalFree(sd);
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sddl_names_exactly_the_current_user() {
        let sddl = user_only_sddl().unwrap();
        assert!(sddl.starts_with("D:P(A;;GA;;;S-1-5-"), "{sddl}");
        assert_eq!(sddl.matches("(A;").count(), 1, "exactly one ACE");
    }
}
