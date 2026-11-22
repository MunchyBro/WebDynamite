//! Safe opening and path binding for the trusted Windows service configuration chain.
//!
//! Opens the fixed trusted root, each descendant directory, and finally the
//! configuration file with reparse-point following disabled for the final
//! component. Handles are retained by the returned owner for the entire
//! validation/read operation. Every opened handle's resolved DOS path must
//! exactly match the lexically normalized requested path; this rejects
//! intermediate junction/symlink redirection as well as final-component links.
//!
//! This is deliberately fail-closed and supports local drive-letter paths only.
//! It does not activate Windows Service mode or read configuration bytes.

use std::path::{Component, Path, PathBuf};

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};

#[cfg(windows)]
pub struct TrustedConfigHandleChain {
    // File owns each native handle. Keep this vector alive until the caller has
    // completed the bounded read, parsing, and ACL policy evaluation.
    handles: Vec<std::fs::File>,
    paths_file_to_root: Vec<PathBuf>,
}

#[cfg(windows)]
impl TrustedConfigHandleChain {
    /// Returns the handle/path pairs expected by the native ACL collector.
    /// The returned raw handles remain valid only while self is alive.
    pub fn security_objects(
        &self,
    ) -> Vec<(windows_sys::Win32::Foundation::HANDLE, PathBuf)> {
        self.handles
            .iter()
            .zip(self.paths_file_to_root.iter())
            .map(|(file, path)| (file.as_raw_handle() as _, path.clone()))
            .collect()
    }

    /// Number of held objects: config file, all parents, and trusted root.
    pub fn len(&self) -> usize {
        self.handles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }
}

/// Opens config_path and every directory up to and including trusted_root.
///
/// Handles are returned in file -> parent -> ... -> root order through
/// security_objects, matching trusted_config_acl_collector. Internally,
/// they are opened root-first so each ancestor is already held while its
/// descendants are opened. Share mode intentionally excludes write/delete to
/// prevent replacement or rename while the chain is being validated.
#[cfg(windows)]
pub fn open_trusted_config_chain(
    config_path: &Path,
    trusted_root: &Path,
) -> Result<TrustedConfigHandleChain, String> {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::{GetLastError, INVALID_HANDLE_VALUE},
        Security::READ_CONTROL,
        Storage::FileSystem::{
            CreateFileW, GetFileInformationByHandleEx, FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
            FileAttributeTagInfo, OPEN_EXISTING,
        },
    };

    let paths = build_chain_paths(config_path, trusted_root)?;
    let mut handles = Vec::with_capacity(paths.len());

    // Root -> leaf: retain each directory handle before opening its child.
    for (path, is_directory) in paths.iter().rev() {
        let wide = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<_>>();
        let mut access = (READ_CONTROL | FILE_READ_ATTRIBUTES) as u32;
        if !is_directory {
            access |= 0x8000_0000; // GENERIC_READ
        }
        let flags = if *is_directory {
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT
        } else {
            FILE_FLAG_OPEN_REPARSE_POINT
        };
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                FILE_SHARE_READ,
                ptr::null(),
                OPEN_EXISTING,
                flags,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return Err(format!(
                "Could not open trusted config object without following reparse points (Windows error {}).",
                unsafe { GetLastError() }
            ));
        }

        // Transfer ownership immediately so every subsequent error closes the
        // just-opened handle as well as all previously opened ancestors.
        let file = unsafe { std::fs::File::from_raw_handle(handle as RawHandle) };

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
            return Err(format!(
                "Could not inspect trusted config object type (Windows error {}).",
                unsafe { GetLastError() }
            ));
        }
        if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err("Trusted config chain contains a reparse point.".into());
        }
        let actual_directory = attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        if actual_directory != *is_directory {
            return Err("Trusted config chain contains an unexpected file/directory type.".into());
        }

        let resolved = final_path_for_handle(handle)?;
        let expected = full_path_without_resolution(path)?;
        if !same_windows_path(&resolved, &expected) {
            return Err("Trusted config handle resolved to a different path than requested; path redirection is not allowed.".into());
        }

        handles.push(file);
    }

    // paths is leaf -> root; handles were opened root -> leaf.
    handles.reverse();
    Ok(TrustedConfigHandleChain {
        handles,
        paths_file_to_root: paths.into_iter().map(|(path, _)| path).collect(),
    })
}

#[cfg(windows)]
fn build_chain_paths(
    config_path: &Path,
    trusted_root: &Path,
) -> Result<Vec<(PathBuf, bool)>, String> {
    if !config_path.is_absolute()
        || !trusted_root.is_absolute()
        || config_path.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || trusted_root.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("Trusted config and root must be normalized absolute paths.".into());
    }

    let config_text = config_path.to_string_lossy();
    let root_text = trusted_root.to_string_lossy();
    if config_text.starts_with("\\\\") || root_text.starts_with("\\\\") {
        return Err("Trusted config paths must use a local drive, not a UNC/device path.".into());
    }
    if !path_is_below(config_path, trusted_root) {
        return Err("Trusted config must be a file below the fixed trusted root.".into());
    }

    let mut chain = vec![(config_path.to_path_buf(), false)];
    let mut current = config_path.parent()
        .ok_or_else(|| "Trusted config file has no parent directory.".to_string())?;
    loop {
        chain.push((current.to_path_buf(), true));
        if same_windows_path(current, trusted_root) {
            break;
        }
        current = current.parent()
            .ok_or_else(|| "Trusted config path does not reach the configured root.".to_string())?;
    }
    Ok(chain))
}

#[cfg(windows)]
fn path_is_below(path: &Path, root: &Path) -> bool {
    let path = normalized_text(path);
    let root = normalized_text(root);
    path.len() > root.len()
        && path.get(..root.len()).is_some_and(|prefix| prefix.eq_ignore_ascii_case(&root))
        && path.as_bytes().get(root.len()).is_some_and(|byte| *byte == b'\\')
}

#[cfg(windows)]
fn same_windows_path(left: &Path, right: &Path) -> bool {
    normalized_text(left).eq_ignore_ascii_case(&normalized_text(right))
}

#[cfg(windows)]
fn normalized_text(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_string()
}

#[cfg(windows)]
fn full_path_without_resolution(path: &Path) -> Result<PathBuf, String> {
    use std::{ffi::OsString, os::windows::ffi::{OsStrExt, OsStringExt}};
    use windows_sys::Win32::{Foundation::GetLastError, Storage::FileSystem::GetFullPathNameW};

    let input = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<_>>();
    let mut buffer = vec![0u16; 32768];
    let length = unsafe {
        GetFullPathNameW(input.as_ptr(), buffer.len() as u32, buffer.as_mut_ptr(), std::ptr::null_mut())
    };
    if length == 0 || length as usize >= buffer.len() {
        return Err(format!("Could not normalize trusted config path (Windows error {}).", unsafe { GetLastError() }));
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length as usize])))
}

#[cfg(windows)]
fn final_path_for_handle(handle: windows_sys::Win32::Foundation::HANDLE) -> Result<PathBuf, String> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    use windows_sys::Win32::{
        Foundation::GetLastError,
        Storage::FileSystem::{GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED, VOLUME_NAME_DOS},
    };

    let mut buffer = vec![0u16; 32768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            handle,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
        )
    };
    if length == 0 || length as usize >= buffer.len() {
        return Err(format!("Could not resolve trusted config handle path (Windows error {}).", unsafe { GetLastError() }));
    }
    let mut value = OsString::from_wide(&buffer[..length as usize]).to_string_lossy().into_owned();
    if let Some(rest) = value.strip_prefix(r"\\?\") {
        value = rest.to_string();
    }
    Ok(PathBuf::from(value))
}

#[cfg(not(windows))]
pub fn open_trusted_config_chain(
    _config_path: &Path,
    _trusted_root: &Path,
) -> Result<(), String> {
    Err("Trusted config handle opening is available only on Windows.".into())
}

#[cfg(test)]
mod tests {

    #[cfg(windows)]
    #[test]
    fn builds_leaf_to_root_chain_for_collector() {
        let chain = super::build_chain_paths(
            std::path::Path::new(r"C:\ProgramData\WebDynamite\config\service.json"),
            std::path::Path::new(r"C:\ProgramData\WebDynamite"),
        ).unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0], (std::path::PathBuf::from(r"C:\ProgramData\WebDynamite\config\service.json"), false));
        assert_eq!(chain[1], (std::path::PathBuf::from(r"C:\ProgramData\WebDynamite\config"), true));
        assert_eq!(chain[2], (std::path::PathBuf::from(r"C:\ProgramData\WebDynamite"), true));
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_handle_opening_fails_closed() {
        let result = super::open_trusted_config_chain(
            std::path::Path::new("/ProgramData/WebDynamite/config/service.json"),
            std::path::Path::new("/ProgramData/WebDynamite"),
        );
        assert!(result.unwrap_err().contains("only on Windows"));
    }
}
