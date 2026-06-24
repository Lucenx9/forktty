use forktty_core::protocol::JsonRpcResponse;

pub(crate) struct SerializedResponse {
    bytes: Vec<u8>,
    encoded_len: usize,
}

impl SerializedResponse {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn encoded_len(&self) -> usize {
        self.encoded_len
    }
}

pub(crate) fn serialize_response(
    response: &JsonRpcResponse,
) -> Result<SerializedResponse, serde_json::Error> {
    let mut bytes = serde_json::to_vec(response)?;
    bytes.push(b'\n');
    let encoded_len = bytes.len();
    Ok(SerializedResponse { bytes, encoded_len })
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
}
