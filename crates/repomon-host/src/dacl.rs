//! Per-user pipe security (PROTOCOL.md §3): an explicit DACL granting access only to the
//! current user, built from SDDL `O:<sid>G:<sid>D:P(A;;GA;;;<sid>)`. The default named-pipe
//! DACL is not acceptable.

use std::ffi::c_void;
use std::ptr::null_mut;

use anyhow::{Context as _, bail};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// An owned self-relative security descriptor restricting access to the current user.
pub struct PipeSecurity {
    descriptor: *mut c_void,
}

// The descriptor is an immutable LocalAlloc'd blob; moving it across threads is fine.
unsafe impl Send for PipeSecurity {}
unsafe impl Sync for PipeSecurity {}

impl PipeSecurity {
    /// Build the descriptor for the process's own user.
    pub fn current_user_only() -> anyhow::Result<Self> {
        let sid = current_user_sid_string()?;
        let sddl = format!("O:{sid}G:{sid}D:P(A;;GA;;;{sid})");
        let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut descriptor: *mut c_void = null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor as *mut *mut c_void as *mut _,
                null_mut(),
            )
        };
        if ok == 0 {
            bail!(
                "ConvertStringSecurityDescriptorToSecurityDescriptorW({sddl}) failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(Self { descriptor })
    }

    /// `SECURITY_ATTRIBUTES` pointing at the descriptor, for
    /// `ServerOptions::create_with_security_attributes_raw`. The returned value borrows
    /// `self`; keep `self` alive for the call.
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor,
            bInheritHandle: 0,
        }
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.descriptor);
        }
    }
}

/// The current process token's user SID, as a string (`S-1-5-21-…`).
fn current_user_sid_string() -> anyhow::Result<String> {
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            bail!(
                "OpenProcessToken failed: {}",
                std::io::Error::last_os_error()
            );
        }
        let result = (|| {
            let mut len = 0u32;
            // First call sizes the buffer; it "fails" with ERROR_INSUFFICIENT_BUFFER.
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len);
            if len == 0 {
                bail!(
                    "GetTokenInformation sizing failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            let mut buf = vec![0u8; len as usize];
            if GetTokenInformation(
                token,
                TokenUser,
                buf.as_mut_ptr() as *mut c_void,
                len,
                &mut len,
            ) == 0
            {
                bail!(
                    "GetTokenInformation failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            let token_user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut sid_w: *mut u16 = null_mut();
            if ConvertSidToStringSidW(token_user.User.Sid, &mut sid_w) == 0 {
                bail!(
                    "ConvertSidToStringSidW failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            let mut end = sid_w;
            while *end != 0 {
                end = end.add(1);
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(
                sid_w,
                end.offset_from(sid_w) as usize,
            ));
            LocalFree(sid_w as *mut c_void);
            Ok(sid)
        })();
        CloseHandle(token);
        result.context("resolve current user SID")
    }
}
