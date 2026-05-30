use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcRequest {
    #[serde(default)]
    pub id: Value,
    pub method: String,
    #[serde(default = "empty_object")]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcResponse {
    #[serde(default)]
    pub id: Value,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcError {
    pub code: String,
    pub message: String,
}

impl JsonRpcResponse {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Value, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(JsonRpcError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ok_response_contains_result_only() {
        let response = JsonRpcResponse::ok(json!("req-1"), json!({ "pong": true }));

        assert_eq!(response.id, json!("req-1"));
        assert!(response.ok);
        assert_eq!(response.result, Some(json!({ "pong": true })));
        assert_eq!(response.error, None);
    }

    #[test]
    fn error_response_contains_error_only() {
        let response = JsonRpcResponse::error(json!("req-2"), "invalid_params", "Bad params");

        assert_eq!(response.id, json!("req-2"));
        assert!(!response.ok);
        assert_eq!(response.result, None);
        assert_eq!(
            response.error,
            Some(JsonRpcError {
                code: "invalid_params".to_string(),
                message: "Bad params".to_string(),
            })
        );
    }

    #[test]
    fn request_defaults_missing_params_to_empty_object() {
        let request: JsonRpcRequest =
            serde_json::from_value(json!({ "id": "req-3", "method": "system.ping" })).unwrap();

        assert_eq!(request.params, json!({}));
    }
}
