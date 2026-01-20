use serde_json::{
    json,
    Value as JsonValue,
};
use sync_types::{
    ErrorPayload,
    ServerMessage as SyncServerMessage,
    StateModification,
};

use crate::ServerMessage;

pub const BINARY_FRAME_TYPE_FULL: u8 = 0;
pub const BINARY_FRAME_TYPE_CHUNK: u8 = 1;

const PACKED_PLACEHOLDER_KEY: &str = "$packed";

struct PackedValueEncoder<'a> {
    packed_values: Vec<&'a [u8]>,
}

impl<'a> PackedValueEncoder<'a> {
    fn new() -> Self {
        Self {
            packed_values: Vec::new(),
        }
    }

    fn encode_value(&mut self, value: &'a packed_value::PackedSyncValue) -> JsonValue {
        let index = self.packed_values.len();
        self.packed_values.push(value.as_bytes());
        json!({ PACKED_PLACEHOLDER_KEY: index })
    }
}

pub fn encode_server_message_binary(message: &ServerMessage) -> anyhow::Result<Vec<u8>> {
    let mut encoder = PackedValueEncoder::new();
    let header = encode_server_message_header(message, &mut encoder);
    let header_bytes = serde_json::to_vec(&header)?;
    let packed_len: usize = encoder
        .packed_values
        .iter()
        .map(|bytes| 4 + bytes.len())
        .sum();
    let total_len = 4 + header_bytes.len() + packed_len;
    let mut output = Vec::with_capacity(total_len);
    output.extend_from_slice(&(header_bytes.len() as u32).to_le_bytes());
    output.extend_from_slice(&header_bytes);
    for bytes in encoder.packed_values {
        output.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        output.extend_from_slice(bytes);
    }
    Ok(output)
}

fn encode_server_message_header<'a>(
    message: &'a ServerMessage,
    encoder: &mut PackedValueEncoder<'a>,
) -> JsonValue {
    match message {
        SyncServerMessage::Transition {
            start_version,
            end_version,
            modifications,
            client_clock_skew,
            server_ts,
        } => json!({
            "type": "Transition",
            "startVersion": JsonValue::from(*start_version),
            "endVersion": JsonValue::from(*end_version),
            "modifications": modifications
                .iter()
                .map(|modification| encode_state_modification(modification, encoder))
                .collect::<Vec<_>>(),
            "clientClockSkew": client_clock_skew,
            "serverTs": server_ts,
        }),
        SyncServerMessage::TransitionChunk {
            chunk,
            part_number,
            total_parts,
            transition_id,
        } => json!({
            "type": "TransitionChunk",
            "chunk": chunk,
            "partNumber": part_number,
            "totalParts": total_parts,
            "transitionId": transition_id,
        }),
        SyncServerMessage::MutationResponse {
            request_id,
            result: Ok(value),
            ts,
            log_lines,
        } => json!({
            "type": "MutationResponse",
            "requestId": request_id,
            "success": true,
            "result": encoder.encode_value(value),
            "ts": ts.map(|ts| u64_to_string(ts.into())),
            "logLines": log_lines,
        }),
        SyncServerMessage::MutationResponse {
            request_id,
            result: Err(error_payload),
            ts,
            log_lines,
        } => {
            let mut response = json!({
                "type": "MutationResponse",
                "requestId": request_id,
                "success": false,
                "result": error_payload.get_message(),
                "ts": ts.map(|ts| u64_to_string(ts.into())),
                "logLines": log_lines,
            });
            if let ErrorPayload::ErrorData { data, .. } = error_payload {
                response["errorData"] = encoder.encode_value(data);
            }
            response
        },
        SyncServerMessage::ActionResponse {
            request_id,
            result: Ok(value),
            log_lines,
        } => json!({
            "type": "ActionResponse",
            "requestId": request_id,
            "success": true,
            "result": encoder.encode_value(value),
            "logLines": log_lines,
        }),
        SyncServerMessage::ActionResponse {
            request_id,
            result: Err(error_payload),
            log_lines,
        } => {
            let mut response = json!({
                "type": "ActionResponse",
                "requestId": request_id,
                "success": false,
                "result": error_payload.get_message(),
                "logLines": log_lines,
            });
            if let ErrorPayload::ErrorData { data, .. } = error_payload {
                response["errorData"] = encoder.encode_value(data);
            }
            response
        },
        SyncServerMessage::AuthError {
            error_message,
            base_version,
            auth_update_attempted,
        } => {
            let mut response = json!({
                "type": "AuthError",
                "error": error_message,
                "baseVersion": base_version,
            });
            if let Some(auth_update_attempted) = auth_update_attempted {
                response["authUpdateAttempted"] = JsonValue::from(*auth_update_attempted);
            }
            response
        },
        SyncServerMessage::FatalError { error_message } => json!({
            "type": "FatalError",
            "error": error_message,
        }),
        SyncServerMessage::Ping => json!({
            "type": "Ping",
        }),
    }
}

fn encode_state_modification<'a>(
    modification: &'a StateModification<packed_value::PackedSyncValue>,
    encoder: &mut PackedValueEncoder<'a>,
) -> JsonValue {
    match modification {
        StateModification::QueryUpdated {
            query_id,
            value,
            log_lines,
            journal,
        } => json!({
            "type": "QueryUpdated",
            "queryId": query_id,
            "value": encoder.encode_value(value),
            "logLines": log_lines,
            "journal": journal,
        }),
        StateModification::QueryFailed {
            query_id,
            error_message,
            log_lines,
            journal,
            error_data,
        } => {
            let mut response = json!({
                "type": "QueryFailed",
                "queryId": query_id,
                "errorMessage": error_message,
                "logLines": log_lines,
                "journal": journal,
            });
            if let Some(error_data) = error_data {
                response["errorData"] = encoder.encode_value(error_data);
            }
            response
        },
        StateModification::QueryRemoved { query_id } => json!({
            "type": "QueryRemoved",
            "queryId": query_id,
        }),
    }
}

fn u64_to_string(x: u64) -> String {
    base64::encode(x.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use std::convert::TryInto;

    use common::value::ConvexValue;
    use sync_types::LogLinesMessage;

    use super::*;

    #[test]
    fn encode_server_message_binary_includes_packed_payload() -> anyhow::Result<()> {
        let packed = packed_value::PackedSyncValue::pack(&ConvexValue::from(42));
        let message = ServerMessage::MutationResponse {
            request_id: 1,
            result: Ok(packed.clone()),
            ts: None,
            log_lines: LogLinesMessage(vec![]),
        };

        let encoded = encode_server_message_binary(&message)?;
        let header_len = u32::from_le_bytes(encoded[0..4].try_into()?) as usize;
        let header_bytes = &encoded[4..4 + header_len];
        let header: JsonValue = serde_json::from_slice(header_bytes)?;
        assert_eq!(header["result"]["$packed"], 0);

        let payload_offset = 4 + header_len;
        let packed_len =
            u32::from_le_bytes(encoded[payload_offset..payload_offset + 4].try_into()?) as usize;
        let packed_bytes = &encoded[payload_offset + 4..payload_offset + 4 + packed_len];
        assert_eq!(packed_len, packed.as_bytes().len());
        assert_eq!(packed_bytes, packed.as_bytes());
        Ok(())
    }
}

