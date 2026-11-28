//! Strict parser and validator for installer-provisioned Windows service identity.
//!
//! This module intentionally does not locate, read, or trust a configuration
//! file by itself. The future SCM host must first verify the file's ACL and
//! provenance using an installer-owned path, then pass its bounded bytes here.
//! Never populate these values from ordinary user-writable environment variables.
use serde::Deserialize;
use std::path::PathBuf;

pub const SERVICE_CONFIG_VERSION: u32 = 1;
pub const MAX_SERVICE_CONFIG_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TrustedServiceConfig {
    pub version: u32,
    pub service_sid: String,
    pub interactive_user_sid: String,
    pub interactive_session_id: u32,
    pub desktop_executable: PathBuf,
    /// SHA-256 Authenticode publisher certificate thumbprint, 64 hex characters.
    pub publisher_sha256: String,
    pub secret_store_root: PathBuf,
    pub data_root: PathBuf,
}

impl TrustedServiceConfig {
    pub fn parse_and_validate(bytes: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() || bytes.len() > MAX_SERVICE_CONFIG_BYTES {
            return Err("Trusted service configuration must be between 1 byte and 16 KiB.".into());
        }
        let config: Self = serde_json::from_slice(bytes)
            .map_err(|_| "Trusted service configuration is not valid supported JSON.".to_string())?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != SERVICE_CONFIG_VERSION {
            return Err("Unsupported trusted service configuration version.".into());
        }
        if !valid_service_sid(&self.service_sid) {
            return Err("Trusted service configuration contains an invalid service SID.".into());
        }
        if !valid_user_sid(&self.interactive_user_sid) {
            return Err("Trusted service configuration contains an invalid interactive user SID.".into());
        }
        if self.interactive_session_id == 0 {
            return Err("An interactive Windows session ID is required.".into());
        }
        if !self.desktop_executable.is_absolute() || !has_exe_extension(&self.desktop_executable)
            || has_dot_path_components(&self.desktop_executable)
        {
            return Err("The pinned desktop executable must be a normalized absolute .exe path.".into());
        }
        if self.publisher_sha256.len() != 64 || !self.publisher_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("The publisher SHA-256 thumbprint must contain exactly 64 hexadecimal characters.".into());
        }
        if self.publisher_sha256.bytes().all(|byte| byte == b'0') {
            return Err("The publisher SHA-256 thumbprint must identify a real signed publisher, not an all-zero placeholder.".into());
        }
        if !self.secret_store_root.is_absolute() || !self.data_root.is_absolute()
            || has_dot_path_components(&self.secret_store_root)
            || has_dot_path_components(&self.data_root)
        {
            return Err("Secret-store and data roots must be normalized absolute paths.".into());
        }
        if paths_overlap(&self.secret_store_root, &self.data_root) {
            return Err("The secret-store root and general data root must be separate, non-overlapping directories.".into());
        }
        Ok(())
    }
}

fn has_exe_extension(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
}

fn has_dot_path_components(path: &std::path::Path) -> bool {
    path.components().any(|component| matches!(
        component,
        std::path::Component::CurDir | std::path::Component::ParentDir
    ))
}

/// Windows paths are case-insensitive. Treat either root containing the other
/// as overlap so secrets cannot be placed inside the general data tree (or vice versa).
fn paths_overlap(left: &std::path::Path, right: &std::path::Path) -> bool {
    let normalize = |path: &std::path::Path| {
        path.to_string_lossy().replace('/', "\\\\")
            .trim_end_matches('\\\\').to_ascii_lowercase()
    };
    let left = normalize(left);
    let right = normalize(right);
    left == right
        || left.strip_prefix(&right).is_some_and(|tail| tail.starts_with('\\\\'))
        || right.strip_prefix(&left).is_some_and(|tail| tail.starts_with('\\\\'))
}

fn valid_service_sid(value: &str) -> bool {
    let parts = value.split('-').collect::<Vec<_>>();
    parts.len() == 9 && parts[0] == "S" && parts[1] == "1" && parts[2] == "5" && parts[3] == "80"
        && parts[4..].iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

fn valid_user_sid(value: &str) -> bool {
    let parts = value.split('-').collect::<Vec<_>>();
    let recognized = (parts.get(2) == Some(&"5") && parts.get(3) == Some(&"21"))
        || (parts.get(2) == Some(&"12") && parts.get(3) == Some(&"1"));
    parts.len() == 8 && parts[0] == "S" && parts[1] == "1" && recognized
        && parts[4..].iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"{
      "version":1,
      "service_sid":"S-1-5-80-123-456-789-101112-131415",
      "interactive_user_sid":"S-1-5-21-123-456-789-1001",
      "interactive_session_id":1,
      "desktop_executable":"C:\\Program Files\\WebDynamite\\WebDynamite.exe",
      "publisher_sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      "secret_store_root":"C:\\ProgramData\\WebDynamite\\Secrets",
      "data_root":"C:\\ProgramData\\WebDynamite\\Data"
    }"#;

    #[test]
    fn rejects_unknown_fields_and_unsupported_versions() {
        let unknown = VALID.replace("\"version\":1", "\"version\":1,\"extra\":true");
        assert!(TrustedServiceConfig::parse_and_validate(unknown.as_bytes()).is_err());
        let version = VALID.replace("\"version\":1", "\"version\":2");
        assert!(TrustedServiceConfig::parse_and_validate(version.as_bytes()).unwrap_err().contains("version"));
    }

    #[test]
    fn rejects_bad_publisher_thumbprints_and_session_ids() {
        let thumb = VALID.replace("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", "not-a-thumbprint");
        assert!(TrustedServiceConfig::parse_and_validate(thumb.as_bytes()).unwrap_err().contains("thumbprint"));
        let placeholder = VALID.replace("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", "0000000000000000000000000000000000000000000000000000000000000000");
        assert!(TrustedServiceConfig::parse_and_validate(placeholder.as_bytes()).unwrap_err().contains("placeholder"));
        let session = VALID.replace("\"interactive_session_id\":1", "\"interactive_session_id\":0");
        assert!(TrustedServiceConfig::parse_and_validate(session.as_bytes()).unwrap_err().contains("session ID"));
    }

    #[test]
    fn rejects_dot_segments_and_overlapping_secret_data_roots() {
        assert!(has_dot_path_components(std::path::Path::new("C:\\\\ProgramData\\\\WebDynamite\\\\..\\\\Secrets")));
        assert!(has_dot_path_components(std::path::Path::new("C:\\\\Program Files\\\\.\\\\WebDynamite.exe")));
        assert!(paths_overlap(
            std::path::Path::new("C:\\\\ProgramData\\\\WebDynamite\\\\Secrets"),
            std::path::Path::new("c:/programdata/webdynamite/secrets/nested")
        ));
        assert!(paths_overlap(
            std::path::Path::new("C:\\\\ProgramData\\\\WebDynamite"),
            std::path::Path::new("C:\\\\ProgramData\\\\WebDynamite\\\\Data")
        ));
        assert!(!paths_overlap(
            std::path::Path::new("C:\\\\ProgramData\\\\WebDynamite\\\\Secrets"),
            std::path::Path::new("C:\\\\ProgramData\\\\WebDynamite\\\\Data")
        ));
        assert!(!paths_overlap(
            std::path::Path::new("C:\\\\ProgramData\\\\WebDynamite\\\\Data"),
            std::path::Path::new("C:\\\\ProgramData\\\\WebDynamite\\\\Database")
        ));
    }

    #[test]
    fn rejects_oversized_configuration_before_parsing() {
        let bytes = vec![b' '; MAX_SERVICE_CONFIG_BYTES + 1];
        assert!(TrustedServiceConfig::parse_and_validate(&bytes).unwrap_err().contains("16 KiB"));
    }

    #[test]
    fn accepts_well_formed_installer_identity_configuration_on_windows() {
        #[cfg(windows)]
        assert!(TrustedServiceConfig::parse_and_validate(VALID.as_bytes()).is_ok());
    }
}
