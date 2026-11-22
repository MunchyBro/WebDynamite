//! Loads trusted service configuration from a fixed, installer-owned Windows path.
//!
//! The loader opens the file and parent directory without following reparse points,
//! checks ownership and a protected allow-list DACL on both handles, then reads a
//! bounded byte stream and delegates schema validation to trusted_service_config.
#[cfg(windows)]
use std::{ffi::OsStr, fs::File, os::{windows::{ffi::OsStrExt, io::FromRawHandle}, raw::c_void}, path::{Path, PathBuf}, ptr};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, LocalFree};
#[cfg(windows)]
use windows_sys::Win32::Security::{Authorization::ConvertStringSidToSidW, GetSecurityDescriptorControl, GetSecurityInfo, GetSecurityDescriptorDacl, GetAclInformation, GetAce, EqualSid, ACL_SIZE_INFORMATION, AclSizeInformation, ACCESS_ALLOWED_ACE, ACCESS_ALLOWED_ACE_TYPE, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, SE_FILE_OBJECT, SE_DACL_PROTECTED};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, OPEN_EXISTING, GENERIC_READ, READ_CONTROL};
#[cfg(windows)]

use crate::trusted_service_config::{TrustedServiceConfig, MAX_SERVICE_CONFIG_BYTES};

/// The path is deliberately fixed. Do not replace this with an environment variable,
/// command-line argument, current-directory lookup, or user-writable settings value.
pub const TRUSTED_SERVICE_CONFIG_PATH: &str = r"C:\ProgramData\WebDynamite\service\service.json";

#[cfg(windows)]
struct OwnedHandle(HANDLE);
#[cfg(windows)]
impl Drop for OwnedHandle {
    fn drop(&mut self) { unsafe { CloseHandle(self.0); } }
}

#[cfg(windows)]
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn open_no_reparse(path: &Path, directory: bool) -> Result<OwnedHandle, String> {
    let path_wide = wide(path);
    let handle = unsafe {
        CreateFileW(
            path_wide.as_ptr(),
            GENERIC_READ | READ_CONTROL,
            FILE_SHARE_READ,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | if directory { FILE_FLAG_BACKUP_SEMANTICS } else { 0 },
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!("Could not open trusted service configuration path: {}", std::io::Error::last_os_error()));
    }
    Ok(OwnedHandle(handle))
}

#[cfg(windows)]
fn sid_matches(actual: *mut c_void, sid_text: &str) -> bool {
    let wide_sid = OsStr::new(sid_text).encode_wide().chain(std::iter::once(0)).collect::<Vec<_>>();
    let mut expected = ptr::null_mut();
    let converted = unsafe { ConvertStringSidToSidW(wide_sid.as_ptr(), &mut expected) };
    if converted == 0 { return false; }
    let equal = unsafe { EqualSid(actual, expected) } != 0;
    unsafe { LocalFree(expected); }
    equal
}

#[cfg(windows)]
fn is_trusted_sid(sid: *mut c_void, service_sid: &str) -> bool {
    sid_matches(sid, "S-1-5-18") || sid_matches(sid, "S-1-5-32-544") || sid_matches(sid, service_sid)
}

#[cfg(windows)]
fn verify_handle_security(handle: HANDLE, service_sid: &str) -> Result<(), String> {
    let mut owner = ptr::null_mut();
    let mut dacl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            handle, SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner, ptr::null_mut(), &mut dacl, ptr::null_mut(), &mut descriptor,
        )
    };
    if status != 0 {
        return Err(format!("Could not inspect trusted configuration security descriptor (Windows error {status})."));
    }
    let result = (|| {
        if owner.is_null() || !is_trusted_sid(owner, service_sid) {
            return Err("Trusted configuration owner is not SYSTEM, Administrators, or the WebDynamite service.".into());
        }
        let mut control = 0u16;
        let mut revision = 0u16;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
            || control & SE_DACL_PROTECTED == 0 {
            return Err("Trusted configuration DACL must be protected from inheritance.".into());
        }
        let mut present = 0;
        let mut defaulted = 0;
        if unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) } == 0
            || present == 0 || dacl.is_null() {
            return Err("Trusted configuration must have an explicit DACL.".into());
        }
        let mut info = ACL_SIZE_INFORMATION { AceCount: 0, AclBytesInUse: 0, AclBytesFree: 0 };
        if unsafe { GetAclInformation(dacl, &mut info as *mut _ as *mut c_void, std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32, AclSizeInformation) } == 0
            || info.AceCount != 3 {
            return Err("Trusted configuration DACL must contain exactly three trusted principals.".into());
        }
        let mut seen_system = false;
        let mut seen_admins = false;
        let mut seen_service = false;
        for index in 0..info.AceCount {
            let mut ace = ptr::null_mut();
            if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
                return Err("Could not inspect trusted configuration DACL entry.".into());
            }
            let header = unsafe { &*(ace as *const windows_sys::Win32::Security::ACE_HEADER) };
            if header.AceType != ACCESS_ALLOWED_ACE_TYPE || header.AceFlags != 0 {
                return Err("Trusted configuration DACL contains an unsupported or inherited ACE.".into());
            }
            let allowed = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
            let sid = unsafe { &allowed.SidStart as *const u32 as *mut c_void };
            if sid_matches(sid, "S-1-5-18") {
                if seen_system { return Err("Trusted configuration DACL contains a duplicate SYSTEM entry.".into()); }
                seen_system = true;
            } else if sid_matches(sid, "S-1-5-32-544") {
                if seen_admins { return Err("Trusted configuration DACL contains a duplicate Administrators entry.".into()); }
                seen_admins = true;
            } else if sid_matches(sid, service_sid) {
                if seen_service { return Err("Trusted configuration DACL contains a duplicate service entry.".into()); }
                seen_service = true;
            } else {
                return Err("Trusted configuration DACL grants access to an untrusted principal.".into());
            }
        }
        if !(seen_system && seen_admins && seen_service) {
            return Err("Trusted configuration DACL must grant access to SYSTEM, Administrators, and this service.".into());
        }
        Ok(())
    })();
    unsafe { LocalFree(descriptor); }
    result
}

#[cfg(windows)]
fn verify_path_object(path: &Path, directory: bool, service_sid: &str) -> Result<OwnedHandle, String> {
    let handle = open_no_reparse(path, directory)?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(handle.0, &mut info) } == 0 {
        return Err(format!("Could not inspect trusted configuration object: {}", std::io::Error::last_os_error()));
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err("Trusted configuration path must not be a reparse point.".into());
    }
    let is_directory = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    if is_directory != directory {
        return Err(if directory { "Trusted configuration parent must be a directory.".into() } else { "Trusted configuration must be a regular file.".into() });
    }
    verify_handle_security(handle.0, service_sid)?;
    Ok(handle)
}

/// Opens only the fixed ProgramData location. Both the containing directory and
/// file must have protected DACLs with no inherited ACEs and no principals beyond
/// SYSTEM, Administrators, and the configured service SID.
#[cfg(windows)]
pub fn load_trusted_service_config(service_sid: &str) -> Result<TrustedServiceConfig, String> {
    let path = PathBuf::from(TRUSTED_SERVICE_CONFIG_PATH);
    let parent = path.parent().ok_or("Trusted configuration path has no parent.")?;
    let _parent_handle = verify_path_object(parent, true, service_sid)?;
    let file_handle = verify_path_object(&path, false, service_sid)?;
    let mut file = unsafe { File::from_raw_handle(file_handle.0 as _) };
    // File now owns the handle; prevent OwnedHandle from closing it a second time.
    std::mem::forget(file_handle);
    let size = file.metadata().map_err(|e| format!("Could not inspect trusted configuration size: {e}"))?.len();
    if size == 0 || size > MAX_SERVICE_CONFIG_BYTES as u64 {
        return Err("Trusted service configuration must be between 1 byte and 16 KiB.".into());
    }
    let mut bytes = Vec::with_capacity(size as usize);
    use std::io::Read;
    file.take((MAX_SERVICE_CONFIG_BYTES + 1) as u64).read_to_end(&mut bytes)
        .map_err(|e| format!("Could not read trusted service configuration: {e}"))?;
    if bytes.len() as u64 != size || bytes.len() > MAX_SERVICE_CONFIG_BYTES {
        return Err("Trusted service configuration changed size while being read.".into());
    }
    let config = TrustedServiceConfig::parse_and_validate(&bytes)?;
    if config.service_sid != service_sid {
        return Err("Trusted configuration service SID does not match the running service identity.".into());
    }
    Ok(config)
}

#[cfg(not(windows))]
pub fn load_trusted_service_config(_service_sid: &str) -> Result<TrustedServiceConfig, String> {
    Err("Trusted Windows service configuration loading is available only on Windows.".into())
}

#[cfg(test)]
mod tests {
    #[cfg(not(windows))]
    #[test]
    fn non_windows_fails_closed() {
        assert!(super::load_trusted_service_config("S-1-5-80-1-2-3-4-5").unwrap_err().contains("only on Windows"));
    }
}
