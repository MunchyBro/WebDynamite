//! Narrow dispatcher connecting authorized desktop IPC requests to the
//! machine-DPAPI secret store. This module does not create or secure the store
//! root; the elevated installer must provision its protected directory first.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::{
    ipc_authorization::authorize_request,
    ipc_protocol::{IpcRequest, IpcResponse, PROTOCOL_VERSION},
    secret_store,
};

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
fn storage_key(website_id: &str, name: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(website_id.as_bytes());
    digest.update([0]);
    digest.update(name.as_bytes());
    format!("{:x}", digest.finalize())
}

/// Handles only the three operations admitted by ipc_authorization.
///
/// Secret values are written directly to the protected DPAPI store and are
/// never returned over IPC. The store root must already have its installer-set
/// restrictive DACL; this dispatcher deliberately does not create it.
pub fn dispatch_secret_request(root: &Path, request: &IpcRequest) -> IpcResponse {
    if let Err(code) = authorize_request(&request.operation, &request.data) {
        return response(request, Err(code));
    }
    let result = match request.operation.as_str() {
        "secret.put" => {
            let website_id = request.data.get("website_id").and_then(serde_json::Value::as_str);
            let name = request.data.get("name").and_then(serde_json::Value::as_str);
            let value = request.data.get("value").and_then(serde_json::Value::as_str);
            match (website_id, name, value) {
                (Some(website_id), Some(name), Some(value)) => {
                    let key = storage_key(website_id, name);
                    secret_store::write_secret(root, &key, value.as_bytes())
                        .map(|_| serde_json::json!({"stored": true}))
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
                    Ok(serde_json::json!({"configured": true}))
                }
                _ => Err("secret_store_unavailable"),
            }
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
    fn storage_key_is_fixed_length_and_scoped_to_website_and_name() {
        let first = storage_key("site-a", "DATABASE_URL");
        assert_eq!(first.len(), 64);
        assert_eq!(first, storage_key("site-a", "DATABASE_URL"));
        assert_ne!(first, storage_key("site-b", "DATABASE_URL"));
        assert_ne!(first, storage_key("site-a", "REDIS_URL"));
    }

    #[test]
    fn status_reports_only_whether_a_store_root_is_configured() {
        let root = std::env::temp_dir().join(format!("webdynamite-ipc-status-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let result = dispatch_secret_request(&root, &request("secret.status", json!({})));
        assert!(result.ok);
        assert_eq!(result.data, Some(json!({"configured": true})));
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn status_fails_closed_for_a_missing_store_root() {
        let root = std::env::temp_dir().join(format!("webdynamite-ipc-missing-{}", uuid::Uuid::new_v4()));
        let result = dispatch_secret_request(&root, &request("secret.status", json!({})));
        assert!(!result.ok);
        assert_eq!(result.error_code.as_deref(), Some("secret_store_unavailable"));
    }

    #[test]
    fn unknown_operations_are_not_dispatched() {
        let result = dispatch_secret_request(Path::new("."), &request("filesystem.read", json!({})));
        assert!(!result.ok);
        assert_eq!(result.error_code.as_deref(), Some("operation_not_authorized"));
    }
}
