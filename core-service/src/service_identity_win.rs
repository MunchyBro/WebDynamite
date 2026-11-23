//! Derive the running Windows service identity from its native process token.
//!
//! Service identity is never accepted from an environment variable or from the
//! configuration file. The enabled service SID must be present in TokenGroups
//! of the current process token before trusted configuration is opened.

#[cfg(windows)]
mod windows_impl {
    use std::{mem, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree, HANDLE},
        Security::{
            GetTokenInformation, OpenProcessToken, SID_AND_ATTRIBUTES, TOKEN_GROUPS,
            TOKEN_QUERY, TOKEN_USER, TokenGroups, TokenUser, SE_GROUP_ENABLED,
        },
        Security::Authorization::ConvertSidToStringSidW,
        System::Threading::GetCurrentProcess,
    };

    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CloseHandle(self.0); }
            }
        }
    }

    /// Identity values read directly from the current service process token.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct NativeServiceIdentity {
        /// User/account SID associated with the service process token.
        pub token_user_sid: String,
        /// Enabled S-1-5-80 service SID group in the process token.
        pub service_sid: String,
    }

    fn token_information(token: HANDLE, class: u32) -> Result<Vec<u64>, String> {
        let mut required = 0u32;
        unsafe {
            GetTokenInformation(token, class, ptr::null_mut(), 0, &mut required);
        }
        if required == 0 || required > 1024 * 1024 {
            return Err("Windows did not return a bounded token information size.".into());
        }
        let mut storage = vec![0u64; (required as usize + 7) / 8];
        let mut returned = 0u32;
        let ok = unsafe {
            GetTokenInformation(
                token,
                class,
                storage.as_mut_ptr().cast(),
                (storage.len() * mem::size_of::<u64>()) as u32,
                &mut returned,
            )
        };
        if ok == 0 || returned == 0 || returned as usize > storage.len() * 8 {
            return Err("Could not read the running service process token.".into());
        }
        Ok(storage)
    }

    fn sid_to_string(sid: *mut core::ffi::c_void) -> Result<String, String> {
        if sid.is_null() {
            return Err("The service process token contains a null SID.".into());
        }
        let mut text = ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
            return Err("Could not convert a service token SID to text.".into());
        }
        let mut length = 0usize;
        unsafe {
            while *text.add(length) != 0 { length += 1; }
        }
        let result = String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
            .map_err(|_| "A service token SID was not valid UTF-16.".to_string());
        unsafe { LocalFree(text.cast()); }
        result
    }

    fn is_service_sid(value: &str) -> bool {
        let parts = value.split('-').collect::<Vec<_>>();
        parts.len() == 9
            && parts[0] == "S"
            && parts[1] == "1"
            && parts[2] == "5"
            && parts[3] == "80"
            && parts[4..].iter().all(|part| {
                !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
            })
    }

    /// Reads TokenUser and the single enabled S-1-5-80 service SID from the
    /// current process token. Fails closed when run outside a service identity.
    pub fn current_service_identity() -> Result<NativeServiceIdentity, String> {
        let mut raw_token: HANDLE = ptr::null_mut();
        let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token) };
        if opened == 0 || raw_token.is_null() {
            return Err("Could not open the current Core Service process token.".into());
        }
        let token = OwnedHandle(raw_token);

        let user_storage = token_information(token.0, TokenUser)?;
        let user = unsafe { &*(user_storage.as_ptr().cast::<TOKEN_USER>()) };
        let token_user_sid = sid_to_string(user.User.Sid)?;

        let group_storage = token_information(token.0, TokenGroups)?;
        if group_storage.len() * mem::size_of::<u64>() < mem::size_of::<TOKEN_GROUPS>() {
            return Err("The service process token group list is truncated.".into());
        }
        let groups = group_storage.as_ptr().cast::<TOKEN_GROUPS>();
        let count = unsafe { (*groups).GroupCount as usize };
        let groups_offset = mem::offset_of!(TOKEN_GROUPS, Groups);
        let available = group_storage.len() * mem::size_of::<u64>();
        if count == 0
            || count > (available.saturating_sub(groups_offset)) / mem::size_of::<SID_AND_ATTRIBUTES>()
        {
            return Err("The service process token group list has an invalid length.".into());
        }

        let first = unsafe { ptr::addr_of!((*groups).Groups).cast::<SID_AND_ATTRIBUTES>() };
        let mut service_sids = Vec::new();
        for index in 0..count {
            let entry = unsafe { &*first.add(index) };
            if entry.Attributes & SE_GROUP_ENABLED == 0 {
                continue;
            }
            let sid = sid_to_string(entry.Sid)?;
            if is_service_sid(&sid) {
                service_sids.push(sid);
            }
        }

        if service_sids.len() != 1 {
            return Err("The current process token must contain exactly one enabled WebDynamite service SID.".into());
        }

        Ok(NativeServiceIdentity {
            token_user_sid,
            service_sid: service_sids.remove(0),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::is_service_sid;

        #[test]
        fn accepts_only_well_formed_nt_service_sid_shape() {
            assert!(is_service_sid("S-1-5-80-1-2-3-4-5"));
            assert!(!is_service_sid("S-1-5-21-1-2-3-1001"));
            assert!(!is_service_sid("S-1-5-80-1-2-3-4"));
            assert!(!is_service_sid("S-1-5-80-a-2-3-4-5"));
        }
    }
}

#[cfg(windows)]
pub use windows_impl::{current_service_identity, NativeServiceIdentity};

#[cfg(not(windows))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeServiceIdentity {
    pub token_user_sid: String,
    pub service_sid: String,
}

#[cfg(not(windows))]
pub fn current_service_identity() -> Result<NativeServiceIdentity, String> {
    Err("Native Windows service-token identity is available only on Windows.".into())
}

/// Loads service configuration only after obtaining the expected service SID
/// from the current process token. This prevents a config file from selecting
/// the identity against which its own ACL is checked.
#[cfg(windows)]
pub fn load_current_service_config(
    config_path: &std::path::Path,
    trusted_root: &std::path::Path,
) -> Result<crate::trusted_service_config::TrustedServiceConfig, String> {
    let identity = current_service_identity()?;
    crate::trusted_service_config_loader::load_trusted_service_config(
        config_path,
        trusted_root,
        &identity.service_sid,
    )
}

#[cfg(not(windows))]
pub fn load_current_service_config(
    _config_path: &std::path::Path,
    _trusted_root: &std::path::Path,
) -> Result<crate::trusted_service_config::TrustedServiceConfig, String> {
    Err("Trusted service configuration requires a native Windows service token.".into())
}
