//! Atomic machine-DPAPI secret files stored beneath an installation-protected directory.
//!
//! The caller MUST provision the root directory and apply
//! `secret_store_acl::secure_empty_directory` before writing secrets. New files
//! inherit that directory's protected DACL. This module never writes plaintext
//! secret bytes to disk; it does not create or repair the root directory ACL.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAGIC: &[u8; 4] = b"WDS1";
const MAX_SECRET_BYTES: usize = 64 * 1024;
const MAX_FILE_BYTES: u64 = (MAX_SECRET_BYTES + 1024) as u64;

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
        })
    {
        return Err("Secret name must contain 1-64 ASCII letters, digits, hyphens or underscores.".into());
    }
    Ok(())
}

fn secret_path(root: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("Could not inspect protected secret-store directory: {error}"))?;
    if !metadata.file_type().is_dir() {
        return Err("Secret-store root must be a real directory, not a file or symbolic link.".into());
    }
    Ok(root.join(format!("{name}.wds")))
}

fn encode_envelope(ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    if ciphertext.is_empty() || ciphertext.len() > MAX_FILE_BYTES as usize - MAGIC.len() {
        return Err("Protected secret payload has an invalid size.".into());
    }
    let mut envelope = Vec::with_capacity(MAGIC.len() + ciphertext.len());
    envelope.extend_from_slice(MAGIC);
    envelope.extend_from_slice(ciphertext);
    Ok(envelope)
}

fn decode_envelope(bytes: &[u8]) -> Result<&[u8], String> {
    if bytes.len() <= MAGIC.len() || bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("Protected secret file has an invalid size.".into());
    }
    if &bytes[..MAGIC.len()] != MAGIC {
        return Err("Protected secret file has an unsupported format.".into());
    }
    Ok(&bytes[MAGIC.len()..])
}

/// Encrypts and atomically replaces a named secret file. The root must already
/// have a protected DACL that grants access only to the service and administrators.
pub fn write_secret(root: &Path, name: &str, plaintext: &[u8]) -> Result<(), String> {
    if plaintext.is_empty() || plaintext.len() > MAX_SECRET_BYTES {
        return Err("Secret must contain 1-65536 bytes.".into());
    }
    let destination = secret_path(root, name)?;
    let ciphertext = crate::secrets::protect_machine(plaintext)?;
    let envelope = encode_envelope(&ciphertext)?;

    // The unique temporary file is created in the same protected directory so
    // it inherits the directory DACL and can be atomically renamed into place.
    let temporary = root.join(format!(".{}.{}.tmp", name, uuid::Uuid::new_v4()));
    let write_result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("Could not create protected secret temporary file: {error}"))?;
        file.write_all(&envelope)
            .map_err(|error| format!("Could not write protected secret: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Could not flush protected secret to disk: {error}"))?;
        drop(file);
        atomic_replace(&temporary, &destination)?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

/// Writes a secret and verifies that the protected value can be read back before
/// reporting success. Migration callers must keep the source credential until this
/// function returns Ok; a failed verification never authorizes source deletion.
pub fn write_secret_verified(root: &Path, name: &str, plaintext: &[u8]) -> Result<(), String> {
    write_secret(root, name, plaintext)?;
    let mut recovered = read_secret(root, name)?;
    let matches = recovered.as_slice() == plaintext;
    recovered.fill(0);
    if !matches {
        return Err("Protected secret verification failed; retain the source credential.".into());
    }
    Ok(())
}

/// Reads and decrypts a named secret. Plaintext exists only in the returned memory buffer.
pub fn read_secret(root: &Path, name: &str) -> Result<Vec<u8>, String> {
    let path = secret_path(root, name)?;
    let mut file = File::open(&path)
        .map_err(|error| format!("Could not open protected secret: {error}"))?;
    let size = file.metadata()
        .map_err(|error| format!("Could not inspect protected secret: {error}"))?.len();
    if size <= MAGIC.len() as u64 || size > MAX_FILE_BYTES {
        return Err("Protected secret file has an invalid size.".into());
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read protected secret: {error}"))?;
    let ciphertext = decode_envelope(&bytes)?;
    crate::secrets::unprotect_machine(ciphertext)
}

/// Deletes a named secret file. Missing files are treated as already deleted.
pub fn delete_secret(root: &Path, name: &str) -> Result<(), String> {
    let path = secret_path(root, name)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not delete protected secret: {error}")),
    }
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source_wide = source.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<_>>();
    let destination_wide = destination.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<_>>();
    let moved = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        return Err(format!("Could not atomically replace protected secret: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination)
        .map_err(|error| format!("Could not atomically replace protected secret: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{decode_envelope, encode_envelope, validate_name, MAGIC};

    #[test]
    fn secret_names_cannot_escape_the_store_directory() {
        for name in ["", "../outside", "a/b", "a\\b", ".", "name.wds", "has space", "é"] {
            assert!(validate_name(name).is_err(), "accepted {name:?}");
        }
        assert!(validate_name("database-url_01").is_ok());
    }

    #[test]
    fn envelope_rejects_unknown_or_truncated_files() {
        assert!(decode_envelope(b"").is_err());
        assert!(decode_envelope(MAGIC).is_err());
        assert!(decode_envelope(b"NOPEciphertext").is_err());
        assert_eq!(decode_envelope(b"WDS1ciphertext").unwrap(), b"ciphertext");
    }

    #[test]
    fn envelope_round_trips_ciphertext_without_modification() {
        let encoded = encode_envelope(b"opaque-dpapi-bytes").unwrap();
        assert_eq!(decode_envelope(&encoded).unwrap(), b"opaque-dpapi-bytes");
    }

    #[cfg(windows)]
    #[test]
    fn encrypted_secret_can_be_created_replaced_read_and_deleted() {
        let root = std::env::temp_dir().join(format!("webdynamite-secret-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        crate::secret_store_acl::secure_empty_directory(
            &root,
            "S-1-5-80-123-456-789-101112-131415",
        ).unwrap();
        super::write_secret_verified(&root, "api-token", b"first secret").unwrap();
        assert_eq!(super::read_secret(&root, "api-token").unwrap(), b"first secret");
        super::write_secret(&root, "api-token", b"replacement secret").unwrap();
        assert_eq!(super::read_secret(&root, "api-token").unwrap(), b"replacement secret");
        super::delete_secret(&root, "api-token").unwrap();
        assert!(super::read_secret(&root, "api-token").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
