use forktty_core::protocol::JsonRpcResponse;
use forktty_core::protocol_limits::OFFICIAL_SOCKET_RESPONSE_MAX_BYTES;

pub(crate) struct SerializedResponse {
    bytes: Vec<u8>,
}

impl SerializedResponse {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn encoded_len(&self) -> usize {
        self.bytes.len()
    }
}

pub(crate) fn serialize_response(
    response: &JsonRpcResponse,
) -> Result<SerializedResponse, serde_json::Error> {
    serialize_response_with_limit(response, OFFICIAL_SOCKET_RESPONSE_MAX_BYTES)
}

fn serialize_response_with_limit(
    response: &JsonRpcResponse,
    max_bytes: usize,
) -> Result<SerializedResponse, serde_json::Error> {
    let encoded = encode_response(response)?;
    if encoded.encoded_len() <= max_bytes {
        return Ok(encoded);
    }

    encode_response(&JsonRpcResponse::error(
        response.id.clone(),
        "response_too_large",
        "Response exceeds maximum encoded size",
    ))
}

fn encode_response(response: &JsonRpcResponse) -> Result<SerializedResponse, serde_json::Error> {
    let mut bytes = serde_json::to_vec(response)?;
    bytes.push(b'\n');
    Ok(SerializedResponse { bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn serialize_response_returns_ndjson_bytes_and_len() {
        let response = JsonRpcResponse::ok(json!(7), json!({"message": "hello"}));
        let encoded = serialize_response(&response).unwrap();

        assert_eq!(encoded.encoded_len(), encoded.as_bytes().len());
        assert_eq!(encoded.as_bytes().last(), Some(&b'\n'));

        let value: Value =
            serde_json::from_slice(&encoded.as_bytes()[..encoded.as_bytes().len() - 1]).unwrap();
        assert_eq!(value["id"], json!(7));
        assert_eq!(value["ok"], json!(true));
        assert_eq!(value["result"], json!({"message": "hello"}));
    }

    #[test]
    fn oversized_response_becomes_same_id_response_too_large() {
        let response =
            JsonRpcResponse::ok(json!("request-42"), json!({"message": "x".repeat(1_024)}));

        let encoded = serialize_response_with_limit(&response, 256).unwrap();
        let value: Value =
            serde_json::from_slice(&encoded.as_bytes()[..encoded.as_bytes().len() - 1]).unwrap();

        assert_eq!(value["id"], json!("request-42"));
        assert_eq!(value["ok"], json!(false));
        assert_eq!(value["error"]["code"], json!("response_too_large"));
        assert_eq!(encoded.as_bytes().last(), Some(&b'\n'));
        assert!(encoded.encoded_len() <= 256);
    }

    #[test]
    fn response_size_limit_counts_the_ndjson_newline() {
        let response = JsonRpcResponse::ok(json!(9), json!({"message": "boundary"}));
        let original = encode_response(&response).unwrap();

        let encoded = serialize_response_with_limit(&response, original.encoded_len() - 1).unwrap();
        let value: Value =
            serde_json::from_slice(&encoded.as_bytes()[..encoded.as_bytes().len() - 1]).unwrap();

        assert_eq!(value["id"], json!(9));
        assert_eq!(value["error"]["code"], json!("response_too_large"));
    }
}
