//! Versioned, bounded message framing for the authenticated local desktop/service IPC channel.
//!
//! This module is transport-agnostic. It does not create a named pipe or grant
//! authorization; callers must authenticate the OS peer before dispatching requests.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};
use zeroize::Zeroize;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_REQUEST_ID_BYTES: usize = 64;
pub const MAX_OPERATION_BYTES: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IpcRequest {
    pub version: u16,
    pub request_id: String,
    pub operation: String,
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IpcResponse {
    pub version: u16,
    pub request_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

fn validate_request(request: &IpcRequest) -> Result<(), String> {
    if request.version != PROTOCOL_VERSION {
        return Err("Unsupported IPC protocol version.".into());
    }
    if request.request_id.is_empty()
        || request.request_id.len() > MAX_REQUEST_ID_BYTES
        || !request.request_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
    {
        return Err("Invalid IPC request identifier.".into());
    }
    if request.operation.is_empty()
        || request.operation.len() > MAX_OPERATION_BYTES
        || !request.operation.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte))
    {
        return Err("Invalid IPC operation name.".into());
    }
    Ok(())
}

/// Reads one big-endian length-prefixed JSON request with a strict allocation bound.
pub fn read_request<R: Read>(reader: &mut R) -> Result<IpcRequest, String> {
    let mut prefix = [0u8; 4];
    reader.read_exact(&mut prefix)
        .map_err(|error| format!("Could not read IPC frame length: {error}"))?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err("IPC frame length is outside the allowed range.".into());
    }
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload)
        .map_err(|error| format!("Could not read complete IPC frame: {error}"))?;
    let parsed = serde_json::from_slice::<IpcRequest>(&payload)
        .map_err(|_| "IPC frame contains invalid or unsupported JSON.".to_string());
    // The framed JSON may contain environment secrets. Clear the temporary
    // serialized copy immediately after deserialization, including parse errors.
    payload.zeroize();
    let request = parsed?;
    validate_request(&request)?;
    Ok(request)
}

/// Writes one bounded big-endian length-prefixed JSON response.
pub fn write_response<W: Write>(writer: &mut W, response: &IpcResponse) -> Result<(), String> {
    if response.version != PROTOCOL_VERSION
        || response.request_id.is_empty()
        || response.request_id.len() > MAX_REQUEST_ID_BYTES
        || !response.request_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
    {
        return Err("Invalid IPC response metadata.".into());
    }
    if response.ok == response.error_code.is_some() {
        return Err("IPC response success and error fields are inconsistent.".into());
    }
    let mut payload = serde_json::to_vec(response)
        .map_err(|_| "Could not encode IPC response.".to_string())?;
    let result = (|| {
        if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
            return Err("IPC response exceeds the maximum frame size.".into());
        }
        let length = u32::try_from(payload.len())
            .map_err(|_| "IPC response length is invalid.".to_string())?;
        writer.write_all(&length.to_be_bytes())
            .and_then(|_| writer.write_all(&payload))
            .and_then(|_| writer.flush())
            .map_err(|error| format!("Could not write IPC response: {error}"))
    })();
    // Responses can contain the short-lived API bootstrap token. Clear the
    // serialized frame on success and on every I/O/validation failure.
    payload.zeroize();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn request() -> IpcRequest {
        IpcRequest {
            version: PROTOCOL_VERSION,
            request_id: "req-01".into(),
            operation: "secret.put".into(),
            data: serde_json::json!({"resource":"website-1","key":"DATABASE_URL","value":"secret"}),
        }
    }

    fn framed(payload: &[u8]) -> Vec<u8> {
        let mut bytes = (payload.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn reads_a_valid_request_frame() {
        let bytes = serde_json::to_vec(&request()).unwrap();
        let decoded = read_request(&mut Cursor::new(framed(&bytes))).unwrap();
        assert_eq!(decoded, request());
    }

    #[test]
    fn rejects_zero_oversized_and_truncated_frames() {
        assert!(read_request(&mut Cursor::new(0u32.to_be_bytes())).is_err());
        assert!(read_request(&mut Cursor::new(((MAX_FRAME_BYTES + 1) as u32).to_be_bytes())).is_err());
        assert!(read_request(&mut Cursor::new(framed(b"{}"))).is_err());
    }

    #[test]
    fn rejects_unknown_versions_and_invalid_operation_names() {
        let mut unsupported = request();
        unsupported.version = PROTOCOL_VERSION + 1;
        let bytes = serde_json::to_vec(&unsupported).unwrap();
        assert!(read_request(&mut Cursor::new(framed(&bytes))).is_err());

        let mut invalid = request();
        invalid.operation = "../secret.read".into();
        let bytes = serde_json::to_vec(&invalid).unwrap();
        assert!(read_request(&mut Cursor::new(framed(&bytes))).is_err());
    }

    #[test]
    fn rejects_unknown_fields_and_duplicate_struct_fields() {
        let unknown = br#"{"version":1,"request_id":"x","operation":"health","data":{},"extra":true}"#;
        assert!(read_request(&mut Cursor::new(framed(unknown))).is_err());
        let duplicate = br#"{"version":1,"request_id":"x","operation":"health","data":{},"data":{}}"#;
        assert!(read_request(&mut Cursor::new(framed(duplicate))).is_err());
    }

    #[test]
    fn writes_a_valid_response_frame() {
        let response = IpcResponse {
            version: PROTOCOL_VERSION,
            request_id: "req-01".into(),
            ok: true,
            data: Some(serde_json::json!({"stored":true})),
            error_code: None,
        };
        let mut output = Vec::new();
        write_response(&mut output, &response).unwrap();
        let length = u32::from_be_bytes(output[..4].try_into().unwrap()) as usize;
        assert_eq!(length, output.len() - 4);
        let decoded: IpcResponse = serde_json::from_slice(&output[4..]).unwrap();
        assert_eq!(decoded, response);
    }

    #[test]
    fn rejects_inconsistent_response_status() {
        let response = IpcResponse {
            version: PROTOCOL_VERSION,
            request_id: "req-01".into(),
            ok: true,
            data: None,
            error_code: Some("denied".into()),
        };
        assert!(write_response(&mut Vec::new(), &response).is_err());
    }
}
