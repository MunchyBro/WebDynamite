//! Narrow dispatcher connecting authorized desktop IPC requests to the
//! machine-DPAPI secret store. Website secrets are write/delete-only; the
//! provisioned API token is handed only to an authenticated desktop peer.
//! The elevated installer must provision the protected store root first.

use std::path::Path;

use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::{
    ipc_authorization::authorize_request,
    ipc_protocol::{IpcRequest, IpcResponse, PROTOCOL_VERSION},
    secret_store,
};

fn take_service_token(bytes: &mut Vec<u8>) -> Result<String, &'static str> {
    let token = std::str::from_utf8(bytes)
        .ok()
        .filter(|value| value.trim().len() >= 32)
        .map(str::to_owned);
    bytes.zeroize();
    token.ok_or("service_token_unavailable")
}

fn response(request: &IpcRequest, result: Result<serde_json::Value, &'static str>) -> IpcResponse {
    match result {
        Ok(data) => IpcResponse {
            version: PROTOCOL_VERSION,
            request_id: request.request_id.clone(),
            ok: true,
            data: Some(data),
            error_code: None,
        },
        Err(code) => IpcResponse {
            version: PROTOCOL_VERSION,
            request_id: request.request_id.clone(),
            ok: false,
            data: None,
            error_code: Some(code.to_string()),
        },
    }
}

/// Derives a fixed-length, website-scoped filename without exposing website IDs
/// or environment variable names in the protected store directory listing.
pub(crate) fn storage_key(website_id: &str, name: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"WebDynamite.SecretStore.Key.v1\0");
    digest.update(website_id.as_bytes());
    digest.update([0]);
    digest.update(name.as_bytes());
    format!("{:x}", digest.finalize())
}
fn database_credential_key(engine: &str, profile_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"WebDynamite.DatabaseCredential.v1\0");
    digest.update(engine.as_bytes());
    digest.update([0]);
    digest.update(profile_id.as_bytes());
    format!("{:x}", digest.finalize())
}

/// Handles only the narrowly allowlisted operations admitted by ipc_authorization.
///
/// Website secret values are written directly to the protected DPAPI store
/// and are never returned over IPC. The API bootstrap token is the sole
/// sensitive read operation and is gated by authenticated peer identity.
/// The store root must already have its installer-set restrictive DACL.
pub fn dispatch_secret_request(root: &Path, request: &mut IpcRequest) -> IpcResponse {
    if let Err(code) = authorize_request(&request.operation, &request.data) {
        if request.operation == "secret.put" {
            if let Some(serde_json::Value::String(value)) = request.data.get_mut("value") {
                value.zeroize();
            }
        }
        return response(request, Err(code));
    }
    let result = match request.operation.as_str() {
        "secret.put" => {
            let website_id = request.data.get("website_id").and_then(serde_json::Value::as_str).map(str::to_owned);
            let name = request.data.get("name").and_then(serde_json::Value::as_str).map(str::to_owned);
            match (website_id, name, request.data.get_mut("value")) {
                (Some(website_id), Some(name), Some(serde_json::Value::String(value))) => {
                    let key = storage_key(&website_id, &name);
                    let stored = secret_store::write_secret_verified(root, &key, value.as_bytes());
                    // The request owns this plaintext string. Wipe it even when
                    // DPAPI, disk I/O, or read-back verification fails.
                    value.zeroize();
                    stored
                        .map(|_| serde_json::json!({"stored": true, "verified": true}))
                        .map_err(|_| "secret_store_unavailable")
                }
                _ => Err("invalid_payload"),
            }
        }
        "secret.delete" => {
            let website_id = request.data.get("website_id").and_then(serde_json::Value::as_str);
            let name = request.data.get("name").and_then(serde_json::Value::as_str);
            match (website_id, name) {
                (Some(website_id), Some(name)) => {
                    let key = storage_key(website_id, name);
                    secret_store::delete_secret(root, &key)
                        .map(|_| serde_json::json!({"deleted": true}))
                        .map_err(|_| "secret_store_unavailable")
                }
                _ => Err("invalid_payload"),
            }
        }
        "secret.status" => {
            match std::fs::symlink_metadata(root) {
                Ok(metadata) if metadata.file_type().is_dir() => {
                    Ok(serde_json::json!({"root_exists": true}))
                }
                _ => Err("secret_store_unavailable"),
            }
        }
        "database-credential.put" => {
            let engine = request.data.get("engine").and_then(serde_json::Value::as_str).map(str::to_owned);
            let profile_id = request.data.get("profile_id").and_then(serde_json::Value::as_str).map(str::to_owned);
            match (engine, profile_id, request.data.get_mut("password")) {
                (Some(engine), Some(profile_id), Some(serde_json::Value::String(password))) => {
                    let key = database_credential_key(&engine, &profile_id);
                    let stored = secret_store::write_secret_verified(root, &key, password.as_bytes());
                    password.zeroize();
                    stored.map(|_| serde_json::json!({"stored":true,"verified":true})).map_err(|_| "secret_store_unavailable")
                }
                _ => Err("invalid_payload"),
            }
        }
        "database-credential.get" => {
            let engine = request.data.get("engine").and_then(serde_json::Value::as_str);
            let profile_id = request.data.get("profile_id").and_then(serde_json::Value::as_str);
            match (engine, profile_id) {
                (Some(engine), Some(profile_id)) => match secret_store::read_secret(root, &database_credential_key(engine, profile_id)) {
                    Ok(mut bytes) => {
                        let password = std::str::from_utf8(&bytes).ok()
                            .filter(|value| !value.is_empty() && value.len() <= 4096).map(str::to_owned);
                        bytes.zeroize();
                        password.map(|value| serde_json::json!({"password":value})).ok_or("database_credential_unavailable")
                    }
                    Err(_) => Err("database_credential_not_found"),
                },
                _ => Err("invalid_payload"),
            }
        }
        "database-credential.delete" => {
            let engine = request.data.get("engine").and_then(serde_json::Value::as_str);
            let profile_id = request.data.get("profile_id").and_then(serde_json::Value::as_str);
            match (engine, profile_id) {
                (Some(engine), Some(profile_id)) => secret_store::delete_secret(root, &database_credential_key(engine, profile_id))
                    .map(|_| serde_json::json!({"deleted":true})).map_err(|_| "secret_store_unavailable"),
                _ => Err("invalid_payload"),
            }
        }
        "service.auth-token" => {
            let mut bytes = match secret_store::read_secret(root, "api-token") {
                Ok(value) => value,
                Err(_) => return response(request, Err("service_token_unavailable")),
            };
            take_service_token(&mut bytes).map(|value| serde_json::json!({"token": value}))
        }
        _ => Err("operation_not_authorized"),
    };
    response(request, result)
}

#[cfg(test)]
mod tests {
    use super::{dispatch_secret_request, storage_key};
    use crate::ipc_protocol::{IpcRequest, PROTOCOL_VERSION};
    use serde_json::json;
    use std::path::Path;

    fn request(operation: &str, data: serde_json::Value) -> IpcRequest {
        IpcRequest {
            version: PROTOCOL_VERSION,
            request_id: "test-1".into(),
            operation: operation.into(),
            data,
        }
    }

    #[test]
    fn service_token_bytes_are_wiped_after_validation() {
        let mut valid = b"abcdefghijklmnopqrstuvwxyz0123456789abcdef".to_vec();
        let token = super::take_service_token(&mut valid).unwrap();
        assert_eq!(token, "abcdefghijklmnopqrstuvwxyz0123456789abcdef");
        assert!(valid.iter().all(|byte| *byte == 0));

        let mut invalid = b"short".to_vec();
        assert_eq!(super::take_service_token(&mut invalid), Err("service_token_unavailable"));
        assert!(invalid.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn storage_key_is_fixed_length_and_scoped_to_website_and_name() {
        let first = storage_key("site-a", "DATABASE_URL");
        assert_eq!(first.len(), 64);
        assert_eq!(first, storage_key("site-a", "DATABASE_URL"));
        assert_ne!(first, storage_key("site-b", "DATABASE_URL"));
        assert_ne!(first, storage_key("site-a", "REDIS_URL"));
    }

    #[test]
    fn database_credential_key_is_domain_separated_and_profile_scoped() {
        let first = super::database_credential_key("mysql", "profile-a");
        assert_eq!(first.len(), 64);
        assert_eq!(first, super::database_credential_key("mysql", "profile-a"));
        assert_ne!(first, super::database_credential_key("postgres", "profile-a"));
        assert_ne!(first, super::database_credential_key("mysql", "profile-b"));
        assert_ne!(first, storage_key("mysql", "profile-a"));
    }

    #[test]
    fn status_reports_root_presence_without_claiming_acl_or_dpapi_readiness() {
        let root = std::env::temp_dir().join(format!("webdynamite-ipc-status-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let result = dispatch_secret_request(&root, &mut request("secret.status", json!({})));
        assert!(result.ok);
        assert_eq!(result.data, Some(json!({"root_exists": true})));
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn status_fails_closed_for_a_missing_store_root() {
        let root = std::env::temp_dir().join(format!("webdynamite-ipc-missing-{}", uuid::Uuid::new_v4()));
        let result = dispatch_secret_request(&root, &mut request("secret.status", json!({})));
        assert!(!result.ok);
        assert_eq!(result.error_code.as_deref(), Some("secret_store_unavailable"));
    }

    #[test]
    fn status_fails_closed_when_the_store_root_is_a_file() {
        let root = std::env::temp_dir().join(format!("webdynamite-ipc-file-{}", uuid::Uuid::new_v4()));
        std::fs::write(&root, b"not a directory").unwrap();
        let result = dispatch_secret_request(&root, &mut request("secret.status", json!({})));
        assert!(!result.ok);
        assert_eq!(result.error_code.as_deref(), Some("secret_store_unavailable"));
        std::fs::remove_file(root).unwrap();
    }


    #[test]
    fn secret_put_wipes_the_owned_request_value_after_storage_attempt() {
        let root = std::env::temp_dir().join(format!("webdynamite-ipc-put-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let mut req = request("secret.put", json!({
            "website_id": "site-a",
            "name": "DATABASE_URL",
            "value": "sensitive-test-value"
        }));
        let _ = dispatch_secret_request(&root, &mut req);
        assert_eq!(req.data.get("value").and_then(serde_json::Value::as_str), Some(""));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unknown_operations_are_not_dispatched() {
        let result = dispatch_secret_request(Path::new("."), &mut request("filesystem.read", json!({})));
        assert!(!result.ok);
        assert_eq!(result.error_code.as_deref(), Some("operation_not_authorized"));
    }
}
