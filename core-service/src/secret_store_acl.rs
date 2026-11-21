//! Applies a restrictive, protected DACL to a newly-created secret-store directory.
//!
//! The directory must be empty. Call this during elevated installation before any
//! secret files are written. The parent directory must already be trusted and
//! protected; this function does not secure ancestors or recursively repair children.

use std::path::Path;

fn valid_service_sid(value: &str) -> bool {
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

#[cfg(windows)]
pub fn secure_empty_directory(path: &Path, service_sid: &str) -> Result<(), String> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SetFileSecurityW, DACL_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION,
        },
    };

    if !valid_service_sid(service_sid) {
        return Err("A valid WebDynamite service SID is required.".into());
    }
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("Could not inspect secret-store directory: {error}"))?;
    if !metadata.is_dir() {
        return Err("Secret-store path must be a directory.".into());
    }
    let mut entries = std::fs::read_dir(path)
        .map_err(|error| format!("Could not inspect secret-store directory: {error}"))?;
    if entries.next().is_some() {
        return Err("Secret-store directory must be empty before its DACL is applied.".into());
    }

    // Protected DACL: only Local System, built-in Administrators, and the
    // dedicated WebDynamite service SID receive full access. No Users/Everyone
    // ACE is present, and inheritance from an untrusted parent is disabled.
    let sddl = format!(
        "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{service_sid})"
    );
    let wide_sddl = OsStr::new(&sddl)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut descriptor = ptr::null_mut();
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide_sddl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        )
    };
    if converted == 0 {
        return Err(format!(
            "Could not build secret-store security descriptor: {}",
            std::io::Error::last_os_error()
        ));
    }

    let wide_path = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<_>>();
    let applied = unsafe {
        SetFileSecurityW(
            wide_path.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    unsafe { LocalFree(descriptor as isize); }
    if applied == 0 {
        return Err(format!(
            "Could not apply restrictive secret-store DACL: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn secure_empty_directory(_path: &Path, _service_sid: &str) -> Result<(), String> {
    Err("Windows secret-store ACL protection is available only on Windows.".into())
}

#[cfg(test)]
mod tests {
    use super::valid_service_sid;

    #[test]
    fn accepts_only_service_sid_shape() {
        assert!(valid_service_sid("S-1-5-80-123-456-789-101112"));
        assert!(!valid_service_sid("S-1-5-80-123"));
        assert!(!valid_service_sid("S-1-5-32-544"));
        assert!(!valid_service_sid("S-1-5-80-123-x-789-101112"));
        assert!(!valid_service_sid("S-1-5-80-123-456-789-101112;OICI"));
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_reports_unsupported_platform() {
        assert!(super::secure_empty_directory(
            std::path::Path::new("."),
            "S-1-5-80-123-456-789-101112"
        ).unwrap_err().contains("only on Windows"));
    }
}
