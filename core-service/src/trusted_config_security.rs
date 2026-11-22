//! Fail-closed policy for installer-collected Windows security metadata.
//!
//! This module deliberately separates Windows handle inspection from policy.
//! A future service entry point must collect each snapshot from an already-open
//! handle (never caller-supplied JSON), reject reparse points at open time, and
//! keep the relevant handles alive through config loading. This evaluator alone
//! does not establish provenance or eliminate path races.
use std::path::{Component, Path, PathBuf};

const SYSTEM_SID: &str = "S-1-5-18";
const ADMINISTRATORS_SID: &str = "S-1-5-32-544";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedObjectSecurity {
    /// Absolute path represented by this handle-derived snapshot.
    pub path: PathBuf,
    pub owner_sid: String,
    pub dacl_present: bool,
    pub dacl_protected: bool,
    pub reparse_point: bool,
    pub is_directory: bool,
    /// SIDs from every ACE in the DACL. Unknown ACE types must be rejected by
    /// the collector rather than omitted from this list.
    pub allowed_ace_sids: Vec<String>,
}

/// Validates the security policy for the config file and every directory from
/// its parent through the fixed installer-owned root, inclusive.
///
/// Snapshots must be ordered file -> parent -> ... -> trusted_root. Only
/// SYSTEM, Administrators, and the configured service SID may appear in the
/// DACL; all three must be present. Owners must be SYSTEM or Administrators.
pub fn validate_trusted_config_security(
    config_path: &Path,
    trusted_root: &Path,
    service_sid: &str,
    snapshots: &[TrustedObjectSecurity],
) -> Result<(), String> {
    if !config_path.is_absolute() || !trusted_root.is_absolute()
        || config_path.components().any(|part| matches!(part, Component::ParentDir))
        || trusted_root.components().any(|part| matches!(part, Component::ParentDir))
    {
        return Err("Trusted config and root must be normalized absolute paths.".into());
    }
    if !valid_service_sid(service_sid) {
        return Err("A valid WebDynamite service SID is required.".into());
    }
    if !path_is_within(config_path, trusted_root) || config_path == trusted_root {
        return Err("Trusted config must be a file below the fixed installer-owned root.".into());
    }

    let expected = expected_path_chain(config_path, trusted_root)?;
    if snapshots.len() != expected.len() {
        return Err("Security metadata must cover the config file and every parent directory through the trusted root.".into());
    }

    let permitted = [SYSTEM_SID, ADMINISTRATORS_SID, service_sid];
    for (index, (snapshot, expected_path)) in snapshots.iter().zip(expected.iter()).enumerate() {
        if !same_path(&snapshot.path, expected_path) {
            return Err("Security metadata paths are incomplete, reordered, or do not match the trusted config path.".into());
        }
        let is_file = index == 0;
        if snapshot.reparse_point {
            return Err("Trusted config path contains a reparse point.".into());
        }
        if snapshot.is_directory == is_file {
            return Err(if is_file {
                "Trusted configuration must be a regular file.".into()
            } else {
                "Every trusted config parent must be a directory.".into()
            });
        }
        if snapshot.owner_sid != SYSTEM_SID && snapshot.owner_sid != ADMINISTRATORS_SID {
            return Err("Trusted config objects must be owned by Local System or built-in Administrators.".into());
        }
        if !snapshot.dacl_present || !snapshot.dacl_protected {
            return Err("Every trusted config object must have a present, protected DACL.".into());
        }
        if snapshot.allowed_ace_sids.is_empty() {
            return Err("Trusted config DACL must not be empty.".into());
        }
        let mut seen = Vec::<&str>::new();
        for sid in &snapshot.allowed_ace_sids {
            if !permitted.iter().any(|allowed| sid.eq_ignore_ascii_case(allowed)) {
                return Err("Trusted config DACL contains an untrusted or unsupported ACE principal.".into());
            }
            if seen.iter().any(|prior| prior.eq_ignore_ascii_case(sid)) {
                return Err("Trusted config DACL contains duplicate principal entries.".into());
            }
            seen.push(sid);
        }
        for required in permitted {
            if !seen.iter().any(|sid| sid.eq_ignore_ascii_case(required)) {
                return Err("Trusted config DACL is missing SYSTEM, Administrators, or the WebDynamite service SID.".into());
            }
        }
    }
    Ok(())
}

fn expected_path_chain(config_path: &Path, trusted_root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    let mut current = config_path.to_path_buf();
    loop {
        paths.push(current.clone());
        if same_path(&current, trusted_root) {
            return Ok(paths);
        }
        if !current.pop() {
            return Err("Trusted config path does not reach the configured root.".into());
        }
    }
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    let path = path.to_string_lossy();
    let root = root.to_string_lossy();
    let path = path.trim_end_matches(['\\\\', '/']);
    let root = root.trim_end_matches(['\\\\', '/']);
    path.len() > root.len()
        && path.get(..root.len()).is_some_and(|prefix| prefix.eq_ignore_ascii_case(&root))
        && path.as_bytes().get(root.len()).is_some_and(|byte| *byte == b'\\\\' || *byte == b'/')
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.to_string_lossy().eq_ignore_ascii_case(&right.to_string_lossy())
}

fn valid_service_sid(value: &str) -> bool {
    let parts = value.split('-').collect::<Vec<_>>();
    parts.len() == 9 && parts[0] == "S" && parts[1] == "1" && parts[2] == "5" && parts[3] == "80"
        && parts[4..].iter().all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVICE: &str = "S-1-5-80-123-456-789-101112-131415";
    const ROOT: &str = "C:\\ProgramData\\WebDynamite";
    const CONFIG: &str = "C:\\ProgramData\\WebDynamite\\config\\service.json";

    fn snapshot(path: &str, is_directory: bool) -> TrustedObjectSecurity {
        TrustedObjectSecurity {
            path: PathBuf::from(path),
            owner_sid: SYSTEM_SID.into(),
            dacl_present: true,
            dacl_protected: true,
            reparse_point: false,
            is_directory,
            allowed_ace_sids: vec![SYSTEM_SID.into(), ADMINISTRATORS_SID.into(), SERVICE.into()],
        }
    }

    fn valid_chain() -> Vec<TrustedObjectSecurity> {
        vec![
            snapshot(CONFIG, false),
            snapshot("C:\\ProgramData\\WebDynamite\\config", true),
            snapshot(ROOT, true),
        ]
    }

    #[test]
    fn accepts_complete_protected_file_and_parent_chain() {
        assert!(validate_trusted_config_security(Path::new(CONFIG), Path::new(ROOT), SERVICE, &valid_chain()).is_ok());
    }

    #[test]
    fn rejects_missing_parent_or_reordered_snapshots() {
        let mut chain = valid_chain();
        chain.pop();
        assert!(validate_trusted_config_security(Path::new(CONFIG), Path::new(ROOT), SERVICE, &chain).is_err());
        let mut chain = valid_chain();
        chain.swap(0, 1);
        assert!(validate_trusted_config_security(Path::new(CONFIG), Path::new(ROOT), SERVICE, &chain).is_err());
    }

    #[test]
    fn rejects_unprotected_acl_reparse_and_untrusted_ace() {
        let mut chain = valid_chain();
        chain[1].dacl_protected = false;
        assert!(validate_trusted_config_security(Path::new(CONFIG), Path::new(ROOT), SERVICE, &chain).is_err());
        let mut chain = valid_chain();
        chain[0].reparse_point = true;
        assert!(validate_trusted_config_security(Path::new(CONFIG), Path::new(ROOT), SERVICE, &chain).is_err());
        let mut chain = valid_chain();
        chain[2].allowed_ace_sids.push("S-1-1-0".into());
        assert!(validate_trusted_config_security(Path::new(CONFIG), Path::new(ROOT), SERVICE, &chain).is_err());
    }

    #[test]
    fn rejects_untrusted_owner_missing_principal_and_wrong_object_type() {
        let mut chain = valid_chain();
        chain[0].owner_sid = "S-1-5-21-1-2-3-1001".into();
        assert!(validate_trusted_config_security(Path::new(CONFIG), Path::new(ROOT), SERVICE, &chain).is_err());
        let mut chain = valid_chain();
        chain[1].allowed_ace_sids.pop();
        assert!(validate_trusted_config_security(Path::new(CONFIG), Path::new(ROOT), SERVICE, &chain).is_err());
        let mut chain = valid_chain();
        chain[0].is_directory = true;
        assert!(validate_trusted_config_security(Path::new(CONFIG), Path::new(ROOT), SERVICE, &chain).is_err());
    }

    #[test]
    fn rejects_path_escape_and_parent_traversal() {
        let chain = valid_chain();
        assert!(validate_trusted_config_security(Path::new("C:\\ProgramData\\WebDynamite-evil\\service.json"), Path::new(ROOT), SERVICE, &chain).is_err());
        assert!(validate_trusted_config_security(Path::new("C:\\ProgramData\\WebDynamite\\..\\evil.json"), Path::new(ROOT), SERVICE, &chain).is_err());
    }
}
