//! Windows DPAPI helpers for machine-scoped secret blobs.
//!
//! Machine scope allows the service and an explicitly authorized desktop process
//! to decrypt the same blob. It does NOT provide caller authorization by itself:
//! callers must protect the containing file with a restrictive Windows DACL.

#[cfg(windows)]
mod platform {
    use std::{ffi::c_void, io, ptr, slice};

    #[repr(C)]
    struct DataBlob {
        size: u32,
        data: *mut u8,
    }

    #[link(name = "Crypt32")]
    extern "system" {
        fn CryptProtectData(
            input: *const DataBlob,
            description: *const u16,
            optional_entropy: *const DataBlob,
            reserved: *mut c_void,
            prompt: *const c_void,
            flags: u32,
            output: *mut DataBlob,
        ) -> i32;

        fn CryptUnprotectData(
            input: *const DataBlob,
            description: *mut *mut u16,
            optional_entropy: *const DataBlob,
            reserved: *mut c_void,
            prompt: *const c_void,
            flags: u32,
            output: *mut DataBlob,
        ) -> i32;
    }

    #[link(name = "Kernel32")]
    extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;
    const CRYPTPROTECT_LOCAL_MACHINE: u32 = 0x4;

    fn input_blob(bytes: &[u8]) -> Result<DataBlob, String> {
        if bytes.is_empty() {
            return Err("Secret data cannot be empty.".into());
        }
        let size = u32::try_from(bytes.len())
            .map_err(|_| "Secret data is too large for Windows DPAPI.".to_string())?;
        Ok(DataBlob {
            size,
            data: bytes.as_ptr() as *mut u8,
        })
    }

    unsafe fn copy_and_free(output: DataBlob) -> Result<Vec<u8>, String> {
        if output.data.is_null() || output.size == 0 {
            if !output.data.is_null() {
                let _ = LocalFree(output.data.cast());
            }
            return Err("Windows DPAPI returned an empty result.".into());
        }
        // DPAPI's output allocation contains plaintext after
        // CryptUnprotectData. Copy it into the caller-owned buffer, then
        // wipe the OS allocation before releasing it to the Windows heap.
        let output_bytes = slice::from_raw_parts_mut(output.data, output.size as usize);
        let result = output_bytes.to_vec();
        output_bytes.fill(0);
        let _ = LocalFree(output.data.cast());
        Ok(result)
    }

    /// Encrypts a non-empty secret so it can be decrypted by authorized users
    /// on this Windows machine. Protect the resulting file with a restrictive DACL.
    pub fn protect_machine(plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let input = input_blob(plaintext)?;
        let mut output = DataBlob {
            size: 0,
            data: ptr::null_mut(),
        };
        let ok = unsafe {
            CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null(),
                CRYPTPROTECT_LOCAL_MACHINE | CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(format!(
                "Windows DPAPI could not protect the secret: {}",
                io::Error::last_os_error()
            ));
        }
        unsafe { copy_and_free(output) }
    }

    /// Decrypts a machine-scoped DPAPI blob on the machine where it was created.
    pub fn unprotect_machine(ciphertext: &[u8]) -> Result<Vec<u8>, String> {
        let input = input_blob(ciphertext)?;
        let mut output = DataBlob {
            size: 0,
            data: ptr::null_mut(),
        };
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(format!(
                "Windows DPAPI could not unprotect the secret: {}",
                io::Error::last_os_error()
            ));
        }
        unsafe { copy_and_free(output) }
    }
}

#[cfg(windows)]
pub use platform::{protect_machine, unprotect_machine};

#[cfg(not(windows))]
pub fn protect_machine(_plaintext: &[u8]) -> Result<Vec<u8>, String> {
    Err("Machine-scoped DPAPI protection is available only on Windows.".into())
}

#[cfg(not(windows))]
pub fn unprotect_machine(_ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    Err("Machine-scoped DPAPI protection is available only on Windows.".into())
}

#[cfg(test)]
mod tests {
    use super::{protect_machine, unprotect_machine};

    #[cfg(windows)]
    #[test]
    fn machine_protection_round_trips_secret_bytes() {
        let secret = b"webdynamite-test-secret-\x00-with-binary-data";
        let protected = protect_machine(secret).unwrap();
        assert_ne!(protected, secret);
        assert_eq!(unprotect_machine(&protected).unwrap(), secret);
    }

    #[cfg(windows)]
    #[test]
    fn tampered_ciphertext_is_rejected() {
        let mut protected = protect_machine(b"tamper-test-secret").unwrap();
        protected[0] ^= 0x80;
        assert!(unprotect_machine(&protected).is_err());
    }

    #[test]
    fn empty_blobs_are_rejected() {
        assert!(protect_machine(&[]).is_err());
        assert!(unprotect_machine(&[]).is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn platform_stub_reports_unsupported_operation() {
        assert!(protect_machine(b"secret").unwrap_err().contains("only on Windows"));
        assert!(unprotect_machine(b"ciphertext").unwrap_err().contains("only on Windows"));
    }
}
