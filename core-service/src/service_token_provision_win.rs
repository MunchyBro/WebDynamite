//! One-time creation of the SCM Core Service API token.
//!
//! Run only after the installer has provisioned and verified the protected
//! ProgramData\WebDynamite\Secrets directory. The token is never printed,
//! passed on a command line, or written as plaintext to disk.

#[cfg(windows)]
pub fn provision_api_token() -> Result<(), String> {
    use zeroize::Zeroizing;
    const ROOT: &str = r"C:\ProgramData\WebDynamite\Secrets";
    const NAME: &str = "api-token";
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut token = Zeroizing::new(Vec::with_capacity(64));
    for uuid in [uuid::Uuid::new_v4(), uuid::Uuid::new_v4()] {
        for byte in uuid.as_bytes() {
            token.push(HEX[(byte >> 4) as usize]);
            token.push(HEX[(byte & 0x0f) as usize]);
        }
    }
    crate::secret_store::create_secret_verified(std::path::Path::new(ROOT), NAME, &token)?;
    Ok(())
}

#[cfg(not(windows))]
pub fn provision_api_token() -> Result<(), String> {
    Err("Windows service API-token provisioning is available only on Windows.".into())
}
