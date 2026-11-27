//! Desktop-side codec for the authenticated Core Service named-pipe protocol.
//!
//! This module deliberately contains no transport or fallback to loopback HTTP.
//! It provides bounded, versioned framing that the Windows pipe client can use.
//! The server remains responsible for peer authentication and operation authorization.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};
use zeroize::Zeroize;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_REQUEST_ID_BYTES: usize = 64;
const MAX_OPERATION_BYTES: usize = 64;

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

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REQUEST_ID_BYTES
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}

fn validate_request(request: &IpcRequest) -> Result<(), String> {
    if request.version != PROTOCOL_VERSION {
        return Err("Unsupported IPC protocol version.".into());
    }
    if !valid_identifier(&request.request_id) {
        return Err("Invalid IPC request identifier.".into());
    }
    if request.operation.is_empty()
        || request.operation.len() > MAX_OPERATION_BYTES
        || !request.operation.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
    {
        return Err("Invalid IPC operation name.".into());
    }
    Ok(())
}

fn validate_response(response: &IpcResponse) -> Result<(), String> {
    if response.version != PROTOCOL_VERSION || !valid_identifier(&response.request_id) {
        return Err("Invalid IPC response metadata.".into());
    }
    if response.ok == response.error_code.is_some() {
        return Err("IPC response success and error fields are inconsistent.".into());
    }
    Ok(())
}

/// Encodes one request as a four-byte big-endian length followed by UTF-8 JSON.
pub fn write_request<W: Write>(writer: &mut W, request: &IpcRequest) -> Result<(), String> {
    validate_request(request)?;
    let mut payload = serde_json::to_vec(request)
        .map_err(|_| "Could not encode IPC request.".to_string())?;
    // The serialized request may contain credentials. Wipe this owned byte
    // buffer after every outcome, including oversized frames and I/O failures.
    let result = (|| {
        if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
            return Err("IPC request exceeds the maximum frame size.".into());
        }
        let length = u32::try_from(payload.len())
            .map_err(|_| "IPC request length is invalid.".to_string())?;
        writer.write_all(&length.to_be_bytes())
            .and_then(|_| writer.write_all(&payload))
            .and_then(|_| writer.flush())
            .map_err(|error| format!("Could not write IPC request: {error}"))
    })();
    payload.zeroize();
    result
}

/// Reads one bounded response frame and rejects unsupported or ambiguous JSON.
pub fn read_response<R: Read>(reader: &mut R) -> Result<IpcResponse, String> {
    let mut prefix = [0u8; 4];
    reader.read_exact(&mut prefix)
        .map_err(|error| format!("Could not read IPC response length: {error}"))?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err("IPC response frame length is outside the allowed range.".into());
    }
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload)
        .map_err(|error| format!("Could not read complete IPC response: {error}"))?;
    let response: IpcResponse = serde_json::from_slice(&payload)
        .map_err(|_| "IPC response contains invalid or unsupported JSON.".to_string())?;
    validate_response(&response)?;
    Ok(response)
}

/// Reads a response and binds it to the request that was sent on this connection.
/// A valid response for another request is still rejected to prevent cross-request
/// confusion if a future transport is reused or receives stale buffered bytes.
pub fn read_response_for_request<R: Read>(
    reader: &mut R,
    expected_request_id: &str,
) -> Result<IpcResponse, String> {
    if !valid_identifier(expected_request_id) {
        return Err("Invalid expected IPC request identifier.".into());
    }
    let response = read_response(reader)?;
    if response.request_id != expected_request_id {
        return Err("IPC response does not match the outstanding request.".into());
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn request() -> IpcRequest {
        IpcRequest {
            version: PROTOCOL_VERSION,
            request_id: "desktop-01".into(),
            operation: "secret.put".into(),
            data: serde_json::json!({
                "website_id": "site-1",
                "name": "DATABASE_URL",
                "value": "temporary-test-value"
            }),
        }
    }

    fn frame(payload: &[u8]) -> Vec<u8> {
        let mut bytes = (payload.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn request_encoding_matches_server_length_prefix_contract() {
        let mut encoded = Vec::new();
        write_request(&mut encoded, &request()).unwrap();
        let length = u32::from_be_bytes(encoded[..4].try_into().unwrap()) as usize;
        assert_eq!(length, encoded.len() - 4);
        let decoded: IpcRequest = serde_json::from_slice(&encoded[4..]).unwrap();
        assert_eq!(decoded, request());
    }

    #[test]
    fn rejects_invalid_request_metadata_before_writing() {
        let mut invalid = request();
        invalid.operation = "../secret.read".into();
        assert!(write_request(&mut Vec::new(), &invalid).is_err());
        let mut invalid = request();
        invalid.version = PROTOCOL_VERSION + 1;
        assert!(write_request(&mut Vec::new(), &invalid).is_err());
    }

    #[test]
    fn reads_a_valid_server_response() {
        let response = IpcResponse {
            version: PROTOCOL_VERSION,
            request_id: "desktop-01".into(),
            ok: true,
            data: Some(serde_json::json!({"stored": true})),
            error_code: None,
        };
        let payload = serde_json::to_vec(&response).unwrap();
        let decoded = read_response(&mut Cursor::new(frame(&payload))).unwrap();
        assert_eq!(decoded, response);
    }

    #[test]
    fn rejects_zero_oversized_truncated_and_ambiguous_response_frames() {
        assert!(read_response(&mut Cursor::new(0u32.to_be_bytes())).is_err());
        assert!(read_response(&mut Cursor::new(((MAX_FRAME_BYTES + 1) as u32).to_be_bytes())).is_err());
        assert!(read_response(&mut Cursor::new(frame(b"{}"))).is_err());
        let duplicate = br#"{"version":1,"request_id":"x","ok":true,"data":{},"data":{}}"#;
        assert!(read_response(&mut Cursor::new(frame(duplicate))).is_err());
    }

    #[test]
    fn response_must_match_the_outstanding_request_id() {
        let response = IpcResponse {
            version: PROTOCOL_VERSION,
            request_id: "different-request".into(),
            ok: true,
            data: None,
            error_code: None,
        };
        let payload = serde_json::to_vec(&response).unwrap();
        assert!(read_response_for_request(
            &mut Cursor::new(frame(&payload)), "desktop-01"
        ).is_err());

        let matching = IpcResponse { request_id: "desktop-01".into(), ..response };
        let payload = serde_json::to_vec(&matching).unwrap();
        assert_eq!(
            read_response_for_request(&mut Cursor::new(frame(&payload)), "desktop-01").unwrap(),
            matching
        );
    }

    #[test]
    fn rejects_invalid_expected_request_id() {
        assert!(read_response_for_request(&mut Cursor::new(Vec::<u8>::new()), "../bad").is_err());
    }

    #[test]
    fn rejects_inconsistent_response_status() {
        let response = IpcResponse {
            version: PROTOCOL_VERSION,
            request_id: "desktop-01".into(),
            ok: true,
            data: None,
            error_code: Some("denied".into()),
        };
        let payload = serde_json::to_vec(&response).unwrap();
        assert!(read_response(&mut Cursor::new(frame(&payload))).is_err());
    }
}
