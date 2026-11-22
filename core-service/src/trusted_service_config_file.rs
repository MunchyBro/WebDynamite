//! Opens a trusted service configuration without following reparse points.
//!
//! The caller must supply the fixed installer-owned path and verify its parent
//! directory/file DACL and ownership before accepting the parsed configuration.
//! This loader enforces a regular-file handle, no reparse-point traversal, and
//! a strict byte limit; it is not a substitute for ACL verification.
use std::{io::Read, path::Path};

#[cfg(windows)]
pub fn read_bounded_config(path: &Path) -> Result<Vec<u8>, String> {
    use std::{os::windows::{ffi::OsStrExt, io::FromRawHandle}, fs::File};
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        Storage::FileSystem::{CreateFileW, GetFileInformationByHandleEx, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_SEQUENTIAL_SCAN, FILE_SHARE_READ, OPEN_EXISTING},
    };
    use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FileAttributeTagInfo};

    let wide = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<_>>();
    let handle = unsafe {
        CreateFileW(wide.as_ptr(), 0x80000000, FILE_SHARE_READ, std::ptr::null(), OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN, std::ptr::null_mut())
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!("Could not open trusted service configuration: {}", std::io::Error::last_os_error()));
    }
    let mut info = FILE_ATTRIBUTE_TAG_INFO { FileAttributes: 0, ReparseTag: 0 };
    let inspected = unsafe {
        GetFileInformationByHandleEx(handle, FileAttributeTagInfo, &mut info as *mut _ as *mut _, std::mem::size_of_val(&info) as u32)
    };
    if inspected == 0 {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(handle); }
        return Err(format!("Could not inspect trusted configuration handle: {}", std::io::Error::last_os_error()));
    }
    if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(handle); }
        return Err("Trusted service configuration must be a regular file, not a directory or reparse point.".into());
    }
    let mut file = unsafe { File::from_raw_handle(handle as _) };
    let size = file.metadata().map_err(|e| format!("Could not inspect trusted configuration size: {e}"))?.len();
    if size == 0 || size > crate::trusted_service_config::MAX_SERVICE_CONFIG_BYTES as u64 {
        return Err("Trusted service configuration must be between 1 byte and 16 KiB.".into());
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take((crate::trusted_service_config::MAX_SERVICE_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Could not read trusted service configuration: {e}"))?;
    if bytes.is_empty() || bytes.len() > crate::trusted_service_config::MAX_SERVICE_CONFIG_BYTES {
        return Err("Trusted service configuration changed size or exceeded 16 KiB while being read.".into());
    }
    Ok(bytes)
}

#[cfg(not(windows))]
pub fn read_bounded_config(_path: &Path) -> Result<Vec<u8>, String> {
    Err("Trusted Windows service configuration loading is available only on Windows.".into())
}
