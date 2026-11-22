//! Operation-level allowlist for authenticated desktop/service IPC requests.
//!
//! Peer identity is necessary but not sufficient: every operation must also
//! pass this policy before application code is invoked. Secret retrieval is
//! deliberately absent; the desktop IPC surface is write/delete/status only.

use serde_json::Value;

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}

fn valid_environment_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && (bytes[0].is_ascii_uppercase() || bytes[0] == b'_')
        && bytes.iter().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_')
        && !matches!(value, "PORT" | "HOST" | "WEBDYNAMITE_API_TOKEN")
}

fn has_exact_fields(data: &Value, expected: &[&str]) -> bool {
    let Some(object) = data.as_object() else { return false; };
    object.len() == expected.len() && expected.iter().all(|field| object.contains_key(*field))
}

fn string_field<'a>(data: &'a Value, field: &str) -> Option<&'a str> {
    data.get(field)?.as_str()
}

/// Authorizes the deliberately narrow desktop IPC surface.
///
/// Supported operations:
/// - `secret.put`: { website_id, name, value }
/// - `secret.delete`: { website_id, name }
/// - `secret.status`: {}
///
/// There is intentionally no `secret.read`, arbitrary filesystem, process,
/// command, or generic RPC operation.
pub fn authorize_request(operation: &str, data: &Value) -> Result<(), &'static str> {
    match operation {
        "secret.put" => {
            if !has_exact_fields(data, &["website_id", "name", "value"]) {
                return Err("invalid_payload");
            }
            let website_id = string_field(data, "website_id").ok_or("invalid_payload")?;
            let name = string_field(data, "name").ok_or("invalid_payload")?;
            let value = string_field(data, "value").ok_or("invalid_payload")?;
            if !valid_identifier(website_id)
                || !valid_environment_name(name)
                || value.is_empty()
                || value.len() > 8192
                || value.contains('\0')
            {
                return Err("invalid_payload");
            }
            Ok(())
        }
        "secret.delete" => {
            if !has_exact_fields(data, &["website_id", "name"]) {
                return Err("invalid_payload");
            }
            let website_id = string_field(data, "website_id").ok_or("invalid_payload")?;
            let name = string_field(data, "name").ok_or("invalid_payload")?;
            if !valid_identifier(website_id) || !valid_environment_name(name) {
                return Err("invalid_payload");
            }
            Ok(())
        }
        "secret.status" => {
            if has_exact_fields(data, &[]) { Ok(()) } else { Err("invalid_payload") }
        }
        _ => Err("operation_not_authorized"),
    }
}

#[cfg(test)]
mod tests {
    use super::authorize_request;
    use serde_json::json;

    #[test]
    fn permits_only_the_narrow_secret_management_operations() {
        assert!(authorize_request("secret.put", &json!({
            "website_id": "site-123",
            "name": "DATABASE_URL",
            "value": "postgres://example"
        })).is_ok());
        assert!(authorize_request("secret.delete", &json!({
            "website_id": "site-123",
            "name": "DATABASE_URL"
        })).is_ok());
        assert!(authorize_request("secret.status", &json!({})).is_ok());
    }

    #[test]
    fn rejects_secret_reads_and_unknown_operations() {
        assert_eq!(authorize_request("secret.read", &json!({"name":"DATABASE_URL"})), Err("operation_not_authorized"));
        assert_eq!(authorize_request("process.spawn", &json!({})), Err("operation_not_authorized"));
        assert_eq!(authorize_request("filesystem.read", &json!({})), Err("operation_not_authorized"));
    }

    #[test]
    fn rejects_extra_fields_and_malformed_secret_payloads() {
        assert!(authorize_request("secret.put", &json!({
            "website_id":"site-1","name":"DATABASE_URL","value":"secret","extra":true
        })).is_err());
        assert!(authorize_request("secret.put", &json!({
            "website_id":"../outside","name":"DATABASE_URL","value":"secret"
        })).is_err());
        assert!(authorize_request("secret.put", &json!({
            "website_id":"site-1","name":"WEBDYNAMITE_API_TOKEN","value":"secret"
        })).is_err());
        assert!(authorize_request("secret.put", &json!({
            "website_id":"site-1","name":"DATABASE_URL","value":""
        })).is_err());
        assert!(authorize_request("secret.delete", &json!({
            "website_id":"site-1","name":"DATABASE_URL","value":"unexpected"
        })).is_err());
        assert!(authorize_request("secret.status", &json!({"debug":true})).is_err());
    }
}
