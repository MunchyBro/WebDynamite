//! Native Windows security-metadata collection for trusted service configuration.
//!
//! This module reads owner/DACL information from already-open Windows handles.
//! It does not open paths itself: the caller must open the config file and every
//! parent directory with no-follow semantics, verify each handle resolves to the
//! intended path, and keep all handles alive until bounded config read and parse.
use std::path::{Path, PathBuf};

use crate::trusted_config_security::{
    validate_trusted_config_security, TrustedObjectSecurity,
};

/// Collects and validates security metadata from caller-owned, already-open
/// handles. The slice must be ordered config file -> each parent -> trusted root.
/// The caller retains ownership of the handles and must keep them open while it
/// reads the config through the corresponding file handle.
#[cfg(windows)]
pub fn collect_and_validate_from_handles(
    config_path: &Path,
    trusted_root: &Path,
    service_sid: &str,
    objects: &[(windows_sys::Win32::Foundation::HANDLE, PathBuf)],
) -> Result<Vec<TrustedObjectSecurity>, String> {
    let mut snapshots = Vec::with_capacity(objects.len());
    for (index, (handle, path)) in objects.iter().enumerate() {
        snapshots.push(collect_one(*handle, path, index != 0)?);
    }
    validate_trusted_config_security(config_path, trusted_root, service_sid, &snapshots)?;
    Ok(snapshots)
}

#[cfg(windows)]
fn collect_one(
    handle: windows_sys::Win32::Foundation::HANDLE,
    path: &Path,
    expected_directory: bool,
) -> Result<TrustedObjectSecurity, String> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, ptr};
    use windows_sys::Win32::{
        Foundation::{GetLastError, LocalFree, INVALID_HANDLE_VALUE},
        Security::{
            GetAce, GetAclInformation, GetSecurityDescriptorControl,
            GetSecurityDescriptorDacl, GetLengthSid, IsValidSid, ACL_SIZE_INFORMATION,
            ACCESS_ALLOWED_ACE, ACE_HEADER,
        },
        Security::Authorization::{ConvertSidToStringSidW, GetSecurityInfo},
        Storage::FileSystem::{
            GetFileInformationByHandleEx, FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FileAttributeTagInfo,
        },
    };

    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err("Trusted config ACL collector received an invalid handle.".into());
    }

    let mut attributes = FILE_ATTRIBUTE_TAG_INFO { FileAttributes: 0, ReparseTag: 0 };
    let attributes_ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileAttributeTagInfo,
            &mut attributes as *mut _ as *mut _,
            std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if attributes_ok == 0 {
        return Err(format!("Could not inspect trusted config handle attributes (Windows error {}).", unsafe { GetLastError() }));
    }
    if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err("Trusted config security handle refers to a reparse point.".into());
    }
    let is_directory = attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    if is_directory != expected_directory {
        return Err("Trusted config security handle has the wrong file/directory type.".into());
    }

    let mut owner = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // SE_FILE_OBJECT = 1; request only owner and DACL, not the SACL.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            1,
            0x0000_0001 | 0x0000_0004,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(format!("Could not read trusted config owner/DACL from handle (Windows error {status})."));
    }

    let result = (|| {
        if owner.is_null() || unsafe { IsValidSid(owner) } == 0 {
            return Err("Trusted config handle returned an invalid owner SID.".into());
        }
        let owner_sid = sid_to_string(owner)?;

        let mut control = 0u16;
        let mut revision = 0u32;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
            return Err(format!("Could not inspect trusted config DACL protection (Windows error {}).", unsafe { GetLastError() }));
        }
        let dacl_protected = control & 0x1000 != 0; // SE_DACL_PROTECTED

        let mut dacl_present = 0i32;
        let mut dacl_defaulted = 0i32;
        let mut descriptor_dacl = ptr::null_mut();
        if unsafe {
            GetSecurityDescriptorDacl(
                descriptor,
                &mut dacl_present,
                &mut descriptor_dacl,
                &mut dacl_defaulted,
            )
        } == 0 {
            return Err(format!("Could not inspect trusted config DACL (Windows error {}).", unsafe { GetLastError() }));
        }
        if dacl_defaulted != 0 {
            return Err("Trusted config DACL must be explicitly installed, not defaulted.".into());
        }
        if dacl_present == 0 || descriptor_dacl.is_null() {
            return Ok(TrustedObjectSecurity {
                path: path.to_path_buf(),
                owner_sid,
                dacl_present: dacl_present != 0,
                dacl_protected,
                reparse_point: false,
                is_directory,
                allowed_ace_sids: Vec::new(),
            });
        }

        let mut acl_info = ACL_SIZE_INFORMATION {
            AceCount: 0,
            AclBytesInUse: 0,
            AclBytesFree: 0,
        };
        if unsafe {
            GetAclInformation(
                descriptor_dacl,
                &mut acl_info as *mut _ as *mut _,
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                2, // AclSizeInformation
            )
        } == 0 {
            return Err(format!("Could not inspect trusted config ACE list (Windows error {}).", unsafe { GetLastError() }));
        }

        if acl_info.AceCount != 3 {
            return Err("Trusted config DACL must contain exactly three installer principals.".into());
        }
        let mut allowed_ace_sids = Vec::with_capacity(acl_info.AceCount as usize);
        for index in 0..acl_info.AceCount {
            let mut ace_ptr = ptr::null_mut();
            if unsafe { GetAce(descriptor_dacl, index, &mut ace_ptr) } == 0 || ace_ptr.is_null() {
                return Err(format!("Could not read trusted config ACE {index} (Windows error {}).", unsafe { GetLastError() }));
            }
            let header = unsafe { &*(ace_ptr as *const ACE_HEADER) };
            if header.AceType != 0 {
                return Err("Trusted config DACL contains a deny, object-specific, callback, or unsupported ACE type.".into());
            }
            if header.AceSize < 16 {
                return Err("Trusted config DACL contains a truncated access-allowed ACE or SID.".into());
            }
            let allowed = unsafe { &*(ace_ptr as *const ACCESS_ALLOWED_ACE) };
            // The installer contract grants full control only to the three
            // trusted principals. No generic/partial masks or extra rights.
            if allowed.Mask != 0x001F_01FF {
                return Err("Trusted config DACL ACE does not match the exact installer full-control mask.".into());
            }
            let expected_flags = if is_directory { 0x03 } else { 0x00 };
            if header.AceFlags != expected_flags {
                return Err("Trusted config DACL ACE has unsupported inheritance or audit flags.".into());
            }
            let sid_ptr = unsafe { (&allowed.SidStart as *const u32).cast_mut().cast() };
            if unsafe { IsValidSid(sid_ptr) } == 0 {
                return Err("Trusted config DACL contains an invalid ACE SID.".into());
            }
            let sid_length = unsafe { GetLengthSid(sid_ptr) } as usize;
            if 8usize.saturating_add(sid_length) > header.AceSize as usize {
                return Err("Trusted config DACL contains a truncated ACE SID.".into());
            }
            allowed_ace_sids.push(sid_to_string(sid_ptr)?);
        }

        Ok(TrustedObjectSecurity {
            path: path.to_path_buf(),
            owner_sid,
            dacl_present: true,
            dacl_protected,
            reparse_point: false,
            is_directory,
            allowed_ace_sids,
        })
    })();

    unsafe { LocalFree(descriptor); }
    result
}

#[cfg(windows)]
fn sid_to_string(sid: windows_sys::Win32::Security::PSID) -> Result<String, String> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, ptr};
    use windows_sys::Win32::{
        Foundation::{GetLastError, LocalFree},
        Security::Authorization::ConvertSidToStringSidW,
    };
    let mut text = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
        return Err(format!("Could not convert trusted config SID to text (Windows error {}).", unsafe { GetLastError() }));
    }
    let mut length = 0usize;
    unsafe { while *text.add(length) != 0 { length += 1; } }
    let value = OsString::from_wide(unsafe { std::slice::from_raw_parts(text, length) })
        .to_string_lossy()
        .into_owned();
    unsafe { LocalFree(text as _); }
    Ok(value)
}

#[cfg(not(windows))]
pub fn collect_and_validate_from_handles(
    _config_path: &Path,
    _trusted_root: &Path,
    _service_sid: &str,
    _objects: &[((), PathBuf)],
) -> Result<Vec<TrustedObjectSecurity>, String> {
    Err("Native trusted config ACL collection is available only on Windows.".into())
}

#[cfg(test)]
mod tests {
    #[cfg(not(windows))]
    #[test]
    fn non_windows_collection_fails_closed() {
        let result = super::collect_and_validate_from_handles(
            std::path::Path::new("/ProgramData/WebDynamite/config.json"),
            std::path::Path::new("/ProgramData/WebDynamite"),
            "S-1-5-80-1-2-3-4-5",
            &[],
        );
        assert!(result.unwrap_err().contains("only on Windows"));
    }
}
