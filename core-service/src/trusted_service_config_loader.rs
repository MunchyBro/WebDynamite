//! Trusted, bounded loading of installer-provisioned Windows service configuration.
//!
//! The caller must supply the service SID obtained from trusted SCM/service-token
//! context, never from the config file or an environment variable. ACL policy is
//! checked against that independently trusted identity before config bytes are read.
use std::{io::Read, path::Path};

use crate::trusted_service_config::{TrustedServiceConfig, MAX_SERVICE_CONFIG_BYTES};

fn read_bounded<R: Read>(reader: R) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(MAX_SERVICE_CONFIG_BYTES);
    reader
        .take((MAX_SERVICE_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read trusted service configuration from its open handle.".to_string())?;
    if bytes.is_empty() || bytes.len() > MAX_SERVICE_CONFIG_BYTES {
        return Err("Trusted service configuration must be between 1 byte and 16 KiB.".into());
    }
    Ok(bytes)
}

/// Opens and validates the complete trusted object chain, checks its native
/// security descriptors, then reads and parses at most 16 KiB from the same
/// already-open config handle. The handle chain remains alive until all checks
/// and parsing finish. The expected SID must come from trusted service identity.
#[cfg(windows)]
pub fn load_trusted_service_config(
    config_path: &Path,
    trusted_root: &Path,
    expected_service_sid: &str,
) -> Result<TrustedServiceConfig, String> {
    let chain = crate::trusted_config_handle_chain::open_trusted_config_chain(
        config_path,
        trusted_root,
    )?;
    let objects = chain.security_objects();

    crate::trusted_config_acl_collector::collect_and_validate_from_handles(
        config_path,
        trusted_root,
        expected_service_sid,
        &objects,
    )?;

    // Read from the exact handle whose owner, DACL, type, and path were checked.
    let bytes = read_bounded(chain.config_file())?;
    let config = TrustedServiceConfig::parse_and_validate(&bytes)?;
    if !config.service_sid.eq_ignore_ascii_case(expected_service_sid) {
        return Err("Trusted service configuration service SID does not match the running service identity.".into());
    }
    Ok(config)
}

#[cfg(not(windows))]
pub fn load_trusted_service_config(
    _config_path: &Path,
    _trusted_root: &Path,
    _expected_service_sid: &str,
) -> Result<TrustedServiceConfig, String> {
    Err("Trusted service configuration loading is available only on Windows.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn bounded_reader_accepts_nonempty_content_at_limit() {
        let bytes = vec![b'x'; MAX_SERVICE_CONFIG_BYTES];
        assert_eq!(read_bounded(Cursor::new(bytes)).unwrap().len(), MAX_SERVICE_CONFIG_BYTES);
    }

    #[test]
    fn bounded_reader_rejects_empty_and_oversized_content() {
        assert!(read_bounded(Cursor::new(Vec::<u8>::new())).is_err());
        let bytes = vec![b'x'; MAX_SERVICE_CONFIG_BYTES + 1];
        assert!(read_bounded(Cursor::new(bytes)).unwrap_err().contains("16 KiB"));
    }

    #[cfg(not(windows))]
    #[test]
    fn loader_fails_closed_off_windows() {
        let result = load_trusted_service_config(
            Path::new("/ProgramData/WebDynamite/config/service.json"),
            Path::new("/ProgramData/WebDynamite"),
            "S-1-5-80-123-456-789-101112-131415",
        );
        assert!(result.unwrap_err().contains("only on Windows"));
    }
}
