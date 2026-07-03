//! Local stdio MCP bridge that maps MCP requests to ForkTTY socket JSON-RPC calls.

use crate::agent_guide;
use crate::socket_cli::CliResult;
use forktty_core::protocol_limits;
use serde_json::{json, Map, Value};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

mod tool_calls;
mod tool_params;
mod tool_specs;
#[cfg(test)]
pub(crate) use tool_calls::build_socket_call_for_test;
#[cfg(test)]
use tool_calls::error_recovery;
use tool_calls::tools_call_result_with_validation;
use tool_specs::tool_specs;

const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
// Per the MCP spec, initialize echoes the client's requested version only when
// the server actually supports it; otherwise it answers with its own latest.
const MCP_SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
const MAX_MCP_MESSAGE_BYTES: usize = protocol_limits::MCP_MESSAGE_MAX_BYTES;

pub(crate) fn run_stdio(socket_path: PathBuf) -> CliResult<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    run_with_io(stdin.lock(), stdout.lock(), socket_path)
}

pub(crate) fn run_with_io<R: BufRead, W: Write>(
    mut reader: R,
    mut writer: W,
    socket_path: PathBuf,
) -> CliResult<()> {
    loop {
        let line = match read_line_bounded(&mut reader, MAX_MCP_MESSAGE_BYTES)? {
            BoundedLine::Eof => break,
            BoundedLine::Oversized => {
                let response = jsonrpc_error(
                    Value::Null,
                    -32700,
                    format!("MCP message exceeds {MAX_MCP_MESSAGE_BYTES} byte limit"),
                );
                write_json_line(&mut writer, &response)?;
                continue;
            }
            BoundedLine::InvalidUtf8(err) => {
                let response = jsonrpc_error(
                    Value::Null,
                    -32700,
                    format!("Parse error: invalid UTF-8: {err}"),
                );
                write_json_line(&mut writer, &response)?;
                continue;
            }
            BoundedLine::Line(line) => line,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(response) = handle_json_line(trimmed, &socket_path) {
            write_json_line(&mut writer, &response)?;
        }
    }
    Ok(())
}

enum BoundedLine {
    Eof,
    Line(String),
    InvalidUtf8(std::string::FromUtf8Error),
    Oversized,
}

/// Reads one `\n`-terminated line, buffering at most `max_bytes` (the
/// trailing newline counts toward the limit, matching the previous
/// `read_line` check). An oversized line is drained without being stored, so
/// a misbehaving client cannot grow this process's memory with the message.
fn read_line_bounded<R: BufRead>(reader: &mut R, max_bytes: usize) -> io::Result<BoundedLine> {
    let mut buf = Vec::new();
    let mut oversized = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if buf.is_empty() && !oversized {
                return Ok(BoundedLine::Eof);
            }
            break;
        }
        let newline = available.iter().position(|&b| b == b'\n');
        let take = newline.map_or(available.len(), |pos| pos + 1);
        if !oversized {
            if buf.len() + take > max_bytes {
                oversized = true;
                buf = Vec::new();
            } else {
                buf.extend_from_slice(&available[..take]);
            }
        }
        reader.consume(take);
        if newline.is_some() {
            break;
        }
    }
    if oversized {
        return Ok(BoundedLine::Oversized);
    }
    Ok(match String::from_utf8(buf) {
        Ok(line) => BoundedLine::Line(line),
        Err(err) => BoundedLine::InvalidUtf8(err),
    })
}

fn write_json_line(writer: &mut impl Write, response: &Value) -> CliResult<()> {
    serde_json::to_writer(&mut *writer, response)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn handle_json_line(line: &str, socket_path: &Path) -> Option<Value> {
    match serde_json::from_str::<Value>(line) {
        Ok(value) => handle_message(value, socket_path),
        Err(err) => Some(jsonrpc_error(
            Value::Null,
            -32700,
            format!("Parse error: {err}"),
        )),
    }
}

fn handle_message(message: Value, socket_path: &Path) -> Option<Value> {
    let Some(object) = message.as_object() else {
        return Some(jsonrpc_error(
            Value::Null,
            -32600,
            "Invalid request: expected object",
        ));
    };
    let id = object.get("id").cloned();
    let response_id = id.clone().unwrap_or(Value::Null);
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return id.map(|_| jsonrpc_error(response_id, -32600, "Invalid request: missing method"));
    };
    if method == "notifications/initialized" {
        return None;
    }
    let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
    let result = match method {
        "initialize" => initialize_result(&params),
        "resources/list" => Ok(resources_list_result()),
        "resources/read" => resources_read_result(&params),
        "prompts/list" => Ok(prompts_list_result()),
        "prompts/get" => prompts_get_result(&params),
        "tools/list" => Ok(tools_list_result()),
        "tools/call" => tools_call_result(&params, socket_path),
        _ => Err(ProtocolError {
            code: -32601,
            message: format!("Method not found: {method}"),
        }),
    };
    let id = id?;
    Some(match result {
        Ok(result) => jsonrpc_ok(id, result),
        Err(err) => jsonrpc_error(id, err.code, err.message),
    })
}

fn initialize_result(params: &Value) -> Result<Value, ProtocolError> {
    let object = params.as_object().ok_or_else(|| ProtocolError {
        code: -32602,
        message: "initialize params must be an object".to_string(),
    })?;
    let protocol_version = object
        .get("protocolVersion")
        .and_then(Value::as_str)
        .filter(|requested| MCP_SUPPORTED_PROTOCOL_VERSIONS.contains(requested))
        .unwrap_or(MCP_PROTOCOL_VERSION);
    Ok(json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "resources": {},
            "prompts": {},
            "tools": {},
        },
        "serverInfo": {
            "name": "forktty",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": agent_guide::mcp_server_instructions(),
    }))
}

fn resources_list_result() -> Value {
    json!({
        "resources": [{
            "uri": agent_guide::OPERATING_GUIDE_URI,
            "name": "forktty_operating_guide",
            "title": "ForkTTY Operating Guide",
            "description": "When coding agents should use ForkTTY panes, workspaces, session resume, worktree, status, and notification tools.",
            "mimeType": "text/plain",
            "annotations": {
                "audience": ["assistant"],
                "priority": 0.8,
            },
        }]
    })
}

fn resources_read_result(params: &Value) -> Result<Value, ProtocolError> {
    let object = params.as_object().ok_or_else(|| ProtocolError {
        code: -32602,
        message: "resources/read params must be an object".to_string(),
    })?;
    let uri = object
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| ProtocolError {
            code: -32602,
            message: "resources/read requires string field uri".to_string(),
        })?;
    if uri != agent_guide::OPERATING_GUIDE_URI {
        return Err(ProtocolError {
            code: -32602,
            message: format!("Unknown resource: {uri}"),
        });
    }
    Ok(json!({
        "contents": [{
            "uri": agent_guide::OPERATING_GUIDE_URI,
            "mimeType": "text/plain",
            "text": agent_guide::operating_guide_text(),
        }]
    }))
}

fn prompts_list_result() -> Value {
    json!({
        "prompts": [{
            "name": agent_guide::OPERATING_GUIDE_PROMPT,
            "title": "ForkTTY Operating Guide",
            "description": "Adds concise ForkTTY tool-use policy to the conversation when coordinating panes, agents, worktrees, or status.",
            "arguments": [],
        }]
    })
}

fn prompts_get_result(params: &Value) -> Result<Value, ProtocolError> {
    let object = params.as_object().ok_or_else(|| ProtocolError {
        code: -32602,
        message: "prompts/get params must be an object".to_string(),
    })?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ProtocolError {
            code: -32602,
            message: "prompts/get requires string field name".to_string(),
        })?;
    if name != agent_guide::OPERATING_GUIDE_PROMPT {
        return Err(ProtocolError {
            code: -32602,
            message: format!("Unknown prompt: {name}"),
        });
    }
    Ok(json!({
        "description": "ForkTTY operating guide for coding agents",
        "messages": [{
            "role": "user",
            "content": {
                "type": "text",
                "text": agent_guide::operating_guide_text(),
            },
        }],
    }))
}

fn tools_list_result() -> Value {
    json!({
        "tools": tool_specs()
            .into_iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": tool.input_schema,
                    "annotations": tool.annotations,
                })
            })
            .collect::<Vec<_>>()
    })
}

fn tools_call_result(params: &Value, socket_path: &Path) -> Result<Value, ProtocolError> {
    let object = params.as_object().ok_or_else(|| ProtocolError {
        code: -32602,
        message: "tools/call params must be an object".to_string(),
    })?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ProtocolError {
            code: -32602,
            message: "tools/call requires string field name".to_string(),
        })?;
    let arguments = object.get("arguments").unwrap_or(&Value::Null);
    let arguments = match arguments {
        Value::Null => Map::new(),
        Value::Object(map) => map.clone(),
        _ => {
            return Err(ProtocolError {
                code: -32602,
                message: "tools/call arguments must be an object".to_string(),
            });
        }
    };
    tools_call_result_with_validation(name, &arguments, socket_path)
}

struct ProtocolError {
    code: i64,
    message: String,
}

fn jsonrpc_ok(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn jsonrpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::with_env;
    use forktty_core::JsonRpcResponse;
    use std::fs;
    use std::io::BufReader;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::thread;

    #[test]
    fn initialize_and_tools_list_follow_json_rpc_protocol() {
        let input = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}
{"jsonrpc":"2.0","id":2,"method":"notifications/initialized","params":{}}
{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}
"#;
        let mut output = Vec::new();
        run_with_io(
            BufReader::new(&input[..]),
            &mut output,
            PathBuf::from("/run/user/1000/forktty.sock"),
        )
        .unwrap();
        let lines = String::from_utf8(output).unwrap();
        let responses = lines
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["jsonrpc"], "2.0");
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(responses[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(responses[0]["result"]["serverInfo"]["name"], "forktty");

        let tools = responses[1]["result"]["tools"].as_array().unwrap();
        let names = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert!(names.contains(&"workspace_list"));
        assert!(names.contains(&"context_snapshot"));
        assert!(names.contains(&"topology_tree"));
        assert!(names.contains(&"surface_read_text"));
        assert!(names.contains(&"surface_capture_tail"));
        assert!(names.contains(&"worktree_create"));
        assert!(names.contains(&"status_set"));
        assert!(tools.iter().any(|tool| {
            tool.get("description")
                .and_then(Value::as_str)
                .is_some_and(|description| description.contains("parallel agent work"))
        }));

        // Every tool must carry annotations, and the hints must reflect the
        // tool's actual effect: list tools are read-only, worktree_remove
        // is destructive, and terminal input can execute commands that touch
        // files, networks, or other external systems.
        let annotation = |name: &str| -> &Value {
            tools
                .iter()
                .find(|tool| tool["name"] == name)
                .map(|tool| &tool["annotations"])
                .unwrap()
        };
        assert!(tools.iter().all(|tool| tool["annotations"].is_object()));
        assert_eq!(annotation("workspace_list")["readOnlyHint"], true);
        assert_eq!(annotation("context_snapshot")["readOnlyHint"], true);
        assert_eq!(annotation("surface_read_text")["readOnlyHint"], true);
        assert_eq!(annotation("surface_capture_tail")["readOnlyHint"], true);
        assert_eq!(annotation("worktree_remove")["destructiveHint"], true);
        assert_eq!(annotation("status_set")["idempotentHint"], true);
        assert_eq!(annotation("surface_send_text")["destructiveHint"], true);
        assert_eq!(annotation("surface_send_text")["openWorldHint"], true);
        assert_eq!(annotation("team_message_dispatch")["destructiveHint"], true);
        assert_eq!(annotation("team_message_dispatch")["openWorldHint"], true);
        assert_eq!(annotation("team_worker_heartbeat")["idempotentHint"], false);
        assert_eq!(annotation("team_message_ack")["idempotentHint"], false);
    }

    #[test]
    fn oversized_message_is_rejected_and_processing_continues() {
        let mut input = vec![b'x'; MAX_MCP_MESSAGE_BYTES + 1];
        input.push(b'\n');
        input.extend_from_slice(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}
"#,
        );
        let mut output = Vec::new();
        run_with_io(
            BufReader::new(&input[..]),
            &mut output,
            PathBuf::from("/run/user/1000/forktty.sock"),
        )
        .unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["error"]["code"], -32700);
        assert!(responses[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("byte limit"));
        assert_eq!(responses[1]["id"], 1);
        assert_eq!(responses[1]["result"]["serverInfo"]["name"], "forktty");
    }

    #[test]
    fn invalid_utf8_message_is_rejected_and_processing_continues() {
        let mut input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"".to_vec();
        input.extend_from_slice(b"\xff}\n");
        input.extend_from_slice(
            br#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}
"#,
        );
        let mut output = Vec::new();
        run_with_io(
            BufReader::new(&input[..]),
            &mut output,
            PathBuf::from("/run/user/1000/forktty.sock"),
        )
        .unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["error"]["code"], -32700);
        assert!(responses[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("invalid UTF-8"));
        assert_eq!(responses[1]["id"], 2);
        assert_eq!(responses[1]["result"]["serverInfo"]["name"], "forktty");
    }

    #[test]
    fn operating_guide_is_exposed_as_mcp_context() {
        let input = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}
{"jsonrpc":"2.0","id":2,"method":"resources/list","params":{}}
{"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":"forktty://agent/operating-guide"}}
{"jsonrpc":"2.0","id":4,"method":"prompts/list","params":{}}
{"jsonrpc":"2.0","id":5,"method":"prompts/get","params":{"name":"forktty_operating_guide"}}
"#;
        let mut output = Vec::new();
        run_with_io(
            BufReader::new(&input[..]),
            &mut output,
            PathBuf::from("/run/user/1000/forktty.sock"),
        )
        .unwrap();
        let responses = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 5);
        assert!(responses[0]["result"]["capabilities"]["resources"].is_object());
        assert!(responses[0]["result"]["capabilities"]["prompts"].is_object());
        assert!(responses[0]["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("For ordinary edits in the current repo, work normally"));

        let resources = responses[1]["result"]["resources"].as_array().unwrap();
        assert!(resources
            .iter()
            .any(|resource| resource["uri"] == "forktty://agent/operating-guide"));
        let resource_text = responses[2]["result"]["contents"][0]["text"]
            .as_str()
            .unwrap();
        assert!(resource_text.contains("SSH remote inventory"));
        assert!(resource_text.contains(
            "For ordinary edits in the current repo, work normally; do not call ForkTTY tools just to edit files."
        ));

        let prompts = responses[3]["result"]["prompts"].as_array().unwrap();
        assert!(prompts
            .iter()
            .any(|prompt| prompt["name"] == "forktty_operating_guide"));
        let prompt_text = responses[4]["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap();
        assert!(prompt_text.contains("Read-only first"));
        assert!(prompt_text.contains("remote_list"));
        assert!(prompt_text.contains("remote_status"));
        assert!(prompt_text.contains("surface_read_text"));
        assert!(prompt_text.contains("surface_send_text"));
    }

    #[test]
    fn tools_call_validates_inputs_without_touching_socket() {
        let input = br#"{"jsonrpc":"2.0","id":"bad","method":"tools/call","params":{"name":"status_set","arguments":{"value":"Running"}}}
"#;
        let mut output = Vec::new();
        run_with_io(
            BufReader::new(&input[..]),
            &mut output,
            PathBuf::from("/run/user/1000/forktty.sock"),
        )
        .unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        let result = &response["result"];
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["code"], "invalid_params");
        assert!(result["structuredContent"]["message"]
            .as_str()
            .unwrap()
            .contains("key is required"));
    }

    #[test]
    fn tools_call_forwards_validated_params_to_socket() {
        let (socket_path, requests_handle) = fake_socket(1, |request| {
            assert_eq!(request["method"], "surface.send_text");
            assert_eq!(request["params"]["surface_id"], "surface-7");
            assert_eq!(request["params"]["text"], "cargo test\n");
            JsonRpcResponse::ok(request["id"].clone(), json!({ "sent": true }))
        });
        let input = br#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"surface_send_text","arguments":{"surface_id":"surface-7","text":"cargo test\n"}}}
"#;
        let mut output = Vec::new();
        run_with_io(BufReader::new(&input[..]), &mut output, socket_path).unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["id"], 9);
        assert_eq!(response["result"]["structuredContent"]["sent"], true);
        assert_eq!(requests_handle.join().unwrap().len(), 1);
    }

    #[test]
    fn error_recovery_suggested_tools_exist() {
        let names = tool_specs()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        let (_, suggested_tool) = error_recovery("precondition_failed").unwrap();
        assert!(
            names.contains(&suggested_tool),
            "suggested_tool {suggested_tool} is not a real tool"
        );
    }

    #[test]
    fn task_strategy_plan_tool_is_read_only() {
        let names = tool_specs()
            .iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();

        assert!(names.contains(&"task_strategy_plan"));
        assert_eq!(annotation("task_strategy_plan")["readOnlyHint"], true);
        assert_eq!(annotation("task_strategy_plan")["openWorldHint"], false);
    }

    #[test]
    fn task_strategy_plan_tool_maps_to_socket_method() {
        let (method, params) = build_socket_call_for_test(
            "task_strategy_plan",
            json!({
                "goal": "Fix the bug and verify tests",
                "task_kind": "focused_bugfix",
                "surface_id": "surface-1",
                "cwd": "/repo/forktty",
                "router_profile": "fast",
                "last_known_good": {
                    "strategy": "solo_with_verify_loop",
                    "harness_id": "claude",
                    "reason": "last successful run"
                },
                "harness_signals": {
                    "codex": {
                        "cooldown": true,
                        "cooldown_reason": "recent quota error"
                    }
                },
                "repo_dirty": true,
                "user_visible": true
            }),
        )
        .unwrap();

        assert_eq!(method, "task.strategy.plan");
        assert_eq!(params["goal"], "Fix the bug and verify tests");
        assert_eq!(params["task_kind"], "focused_bugfix");
        assert_eq!(params["surface_id"], "surface-1");
        assert_eq!(params["cwd"], "/repo/forktty");
        assert_eq!(params["router_profile"], "fast");
        assert_eq!(
            params["last_known_good"]["strategy"],
            "solo_with_verify_loop"
        );
        assert_eq!(params["last_known_good"]["harness_id"], "claude");
        assert_eq!(params["last_known_good"]["reason"], "last successful run");
        assert_eq!(params["harness_signals"]["codex"]["cooldown"], true);
        assert_eq!(
            params["harness_signals"]["codex"]["cooldown_reason"],
            "recent quota error"
        );
        assert_eq!(params["repo_dirty"], true);
        assert_eq!(params["likely_user_visible_change"], true);
    }

    #[test]
    fn task_strategy_plan_surface_arg_overrides_env_workspace_target() {
        let (method, params) = with_env(
            &[
                ("FORKTTY_WORKSPACE_ID", Some("workspace-env")),
                ("FORKTTY_SURFACE_ID", Some("surface-env")),
            ],
            || {
                build_socket_call_for_test(
                    "task_strategy_plan",
                    json!({
                        "goal": "Inspect repo",
                        "surface_id": "surface-explicit"
                    }),
                )
                .unwrap()
            },
        );

        assert_eq!(method, "task.strategy.plan");
        assert_eq!(params["surface_id"], "surface-explicit");
        assert!(params.get("workspace_id").is_none());
    }

    #[test]
    fn task_strategy_tools_reject_blank_goal_at_mcp_boundary() {
        assert!(build_socket_call_for_test("task_strategy_plan", json!({"goal": "  "})).is_err());
        assert!(build_socket_call_for_test(
            "task_strategy_apply",
            json!({
                "run_id": "router-run-1",
                "goal": "  ",
                "plan": {}
            }),
        )
        .is_err());
    }

    #[test]
    fn task_strategy_apply_tool_is_mutating() {
        let names = tool_specs()
            .iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();

        assert!(names.contains(&"task_strategy_apply"));
        assert_eq!(annotation("task_strategy_apply")["readOnlyHint"], false);
        assert_eq!(annotation("task_strategy_apply")["destructiveHint"], true);
        assert_eq!(annotation("task_strategy_apply")["idempotentHint"], false);
        assert_eq!(annotation("task_strategy_apply")["openWorldHint"], true);
    }

    #[test]
    fn task_strategy_apply_tool_maps_to_socket_method() {
        let plan = json!({
            "task_class": "feature_implementation",
            "strategy": "implementer_plus_reviewer",
            "layers": {
                "workflow": true,
                "team": true,
                "loop_metadata": true,
                "worktree": false,
                "feed": true,
                "mcp": true,
                "hooks": true
            },
            "assignments": [
                {"role": "implementer", "harness_id": "codex", "reason": "ready"}
            ],
            "approvals": ["start_run"],
            "reasons": ["classified task as FeatureImplementation"],
            "safety_notes": ["visible setup only"]
        });
        let (method, params) = build_socket_call_for_test(
            "task_strategy_apply",
            json!({
                "run_id": "router-run-1",
                "workspace_id": "w1",
                "leader_surface_id": "s1",
                "cwd": "/repo/forktty",
                "goal": "Implement the router",
                "plan": plan,
                "approved": ["start_run"],
                "approval_id": "task-strategy:abcdef0123456789:approvals:start_run",
                "request_approval": false
            }),
        )
        .unwrap();

        assert_eq!(method, "task.strategy.apply");
        assert_eq!(params["run_id"], "router-run-1");
        assert_eq!(params["workspace_id"], "w1");
        assert_eq!(params["leader_surface_id"], "s1");
        assert_eq!(params["cwd"], "/repo/forktty");
        assert_eq!(params["approved"], json!(["start_run"]));
        assert_eq!(
            params["approval_id"],
            "task-strategy:abcdef0123456789:approvals:start_run"
        );
        assert_eq!(params["request_approval"], false);
        assert_eq!(params["plan"], plan);
    }

    #[test]
    fn orchestration_cleanup_tool_maps_to_socket_method() {
        let names = tool_specs()
            .iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"orchestration_cleanup"));
        assert_eq!(annotation("orchestration_cleanup")["readOnlyHint"], false);
        assert_eq!(annotation("orchestration_cleanup")["destructiveHint"], true);

        let (method, params) = build_socket_call_for_test(
            "orchestration_cleanup",
            json!({
                "workspace_id": "workspace-1",
                "apply": true
            }),
        )
        .unwrap();

        assert_eq!(method, "orchestration.cleanup");
        assert_eq!(params["workspace_id"], "workspace-1");
        assert_eq!(params["apply"], true);
        assert!(params.get("dry_run").is_none());
    }

    #[test]
    fn task_strategy_apply_tool_defaults_to_env_workspace_and_surface() {
        let plan = json!({
            "task_class": "feature_implementation",
            "strategy": "implementer_plus_reviewer",
            "layers": {
                "workflow": true,
                "team": true,
                "loop_metadata": true,
                "worktree": false,
                "feed": true,
                "mcp": true,
                "hooks": true
            },
            "assignments": [
                {"role": "implementer", "harness_id": "codex", "reason": "ready"}
            ],
            "approvals": ["start_run"],
            "reasons": ["classified task as FeatureImplementation"],
            "safety_notes": ["visible setup only"]
        });
        let (method, params) = with_env(
            &[
                ("FORKTTY_WORKSPACE_ID", Some("workspace-env")),
                ("FORKTTY_SURFACE_ID", Some("surface-env")),
            ],
            || {
                build_socket_call_for_test(
                    "task_strategy_apply",
                    json!({
                        "run_id": "router-run-1",
                        "goal": "Implement the router",
                        "plan": plan
                    }),
                )
                .unwrap()
            },
        );

        assert_eq!(method, "task.strategy.apply");
        assert_eq!(params["workspace_id"], "workspace-env");
        assert_eq!(params["leader_surface_id"], "surface-env");
    }

    #[test]
    fn task_strategy_apply_tool_can_request_approval_without_approved_ids() {
        let plan = json!({
            "task_class": "feature_implementation",
            "strategy": "implementer_plus_reviewer",
            "layers": {
                "workflow": true,
                "team": true,
                "loop_metadata": true,
                "worktree": false,
                "feed": true,
                "mcp": true,
                "hooks": true
            },
            "assignments": [
                {"role": "implementer", "harness_id": "codex", "reason": "ready"}
            ],
            "approvals": ["start_run"],
            "reasons": ["classified task as FeatureImplementation"],
            "safety_notes": ["visible setup only"]
        });
        let (method, params) = build_socket_call_for_test(
            "task_strategy_apply",
            json!({
                "run_id": "router-run-1",
                "goal": "Implement the router",
                "plan": plan,
                "request_approval": true
            }),
        )
        .unwrap();

        assert_eq!(method, "task.strategy.apply");
        assert_eq!(params["request_approval"], true);
        assert!(params.get("approved").is_none());
    }

    #[test]
    fn agent_list_tool_maps_to_socket_agent_list() {
        let (method, params) =
            build_socket_call_for_test("agent_list", json!({"workspace_name": "main"})).unwrap();

        assert_eq!(method, "agent.list");
        assert_eq!(params["workspace_name"], "main");
    }

    #[test]
    fn agent_health_tool_maps_to_socket_agent_health() {
        let (method, params) =
            build_socket_call_for_test("agent_health", json!({"workspace_id": "w1"})).unwrap();

        assert_eq!(method, "agent.health");
        assert_eq!(params["workspace_id"], "w1");
    }

    #[test]
    fn agent_reclaim_plan_tool_maps_to_socket_agent_reclaim_plan() {
        let (method, params) = build_socket_call_for_test(
            "agent_reclaim_plan",
            json!({"workspace_id": "w1", "min_idle_ms": 5000}),
        )
        .unwrap();

        assert_eq!(method, "agent.reclaim.plan");
        assert_eq!(params["workspace_id"], "w1");
        assert_eq!(params["min_idle_ms"], 5000);
    }

    #[test]
    fn agent_hibernate_tool_maps_to_socket_agent_hibernate() {
        let (method, params) = build_socket_call_for_test(
            "agent_hibernate",
            json!({"surface_id": "surface-1", "min_idle_ms": 5000}),
        )
        .unwrap();

        assert_eq!(method, "agent.hibernate");
        assert_eq!(params["surface_id"], "surface-1");
        assert_eq!(params["min_idle_ms"], 5000);
    }

    #[test]
    fn agent_reclaim_tool_maps_to_socket_agent_reclaim() {
        let (method, params) = build_socket_call_for_test(
            "agent_reclaim",
            json!({"workspace_id": "w1", "min_idle_ms": 5000, "limit": 3}),
        )
        .unwrap();

        assert_eq!(method, "agent.reclaim");
        assert_eq!(params["workspace_id"], "w1");
        assert_eq!(params["min_idle_ms"], 5000);
        assert_eq!(params["limit"], 3);
    }

    #[test]
    fn agent_resume_tool_maps_to_socket_agent_resume() {
        let (method, params) =
            build_socket_call_for_test("agent_resume", json!({"surface_id": "surface-1"})).unwrap();

        assert_eq!(method, "agent.resume");
        assert_eq!(params["surface_id"], "surface-1");
    }

    #[test]
    fn remote_tools_are_advertised_read_only_and_map_to_socket_methods() {
        let specs = tool_specs();
        for name in ["remote_list", "remote_status"] {
            let spec = specs.iter().find(|tool| tool.name == name).unwrap();
            assert_eq!(spec.annotations["readOnlyHint"], true);
            assert_eq!(spec.annotations["openWorldHint"], false);
        }

        let (method, params) =
            build_socket_call_for_test("remote_list", json!({"workspace_name": "prod"})).unwrap();
        assert_eq!(method, "remote.list");
        assert_eq!(params["workspace_name"], "prod");

        let (method, params) =
            build_socket_call_for_test("remote_status", json!({"surface_id": "s1"})).unwrap();
        assert_eq!(method, "remote.status");
        assert_eq!(params["surface_id"], "s1");
    }

    #[test]
    fn tool_schemas_express_handler_required_alternatives() {
        let specs = tool_specs();
        let workflow_evidence = specs
            .iter()
            .find(|tool| tool.name == "workflow_evidence_add")
            .unwrap();
        let worktree_status = specs
            .iter()
            .find(|tool| tool.name == "worktree_status")
            .unwrap();

        assert!(workflow_evidence.input_schema["anyOf"]
            .as_array()
            .unwrap()
            .iter()
            .any(|schema| schema["required"] == json!(["text"])));
        assert!(workflow_evidence.input_schema["anyOf"]
            .as_array()
            .unwrap()
            .iter()
            .any(|schema| schema["required"] == json!(["path"])));
        assert!(worktree_status.input_schema["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .any(|schema| schema["required"] == json!(["path"])));
        assert!(worktree_status.input_schema["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .any(|schema| schema["required"] == json!(["cwd"])));
    }

    #[test]
    fn team_tools_map_to_socket_team_methods() {
        let (method, params) = build_socket_call_for_test(
            "team_upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": "w1",
                "leader_surface_id": "s1",
                "name": "Launch",
                "goal": "ship runtime"
            }),
        )
        .unwrap();
        assert_eq!(method, "team.upsert");
        assert_eq!(params["team_id"], "team-1");
        assert_eq!(params["workspace_id"], "w1");
        assert_eq!(params["leader_surface_id"], "s1");
        assert_eq!(params["goal"], "ship runtime");

        let (method, params) = with_env(
            &[
                ("FORKTTY_SURFACE_ID", Some(" surface-env ")),
                ("FORKTTY_WORKSPACE_ID", Some(" workspace-env ")),
            ],
            || build_socket_call_for_test("team_upsert", json!({"team_id": "team-env"})).unwrap(),
        );
        assert_eq!(method, "team.upsert");
        assert_eq!(params["team_id"], "team-env");
        assert_eq!(params["leader_surface_id"], "surface-env");
        assert!(params.get("workspace_id").is_none());

        let (method, params) = build_socket_call_for_test(
            "team_worker_heartbeat",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "status": "running",
                "assigned_task_id": "task-1"
            }),
        )
        .unwrap();
        assert_eq!(method, "team.worker.heartbeat");
        assert_eq!(params["assigned_task_id"], "task-1");

        let (method, params) = build_socket_call_for_test(
            "team_worker_launch",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-2",
                "agent": "codex",
                "cwd": "/repo/forktty",
                "args": ["--model", "test"]
            }),
        )
        .unwrap();
        assert_eq!(method, "team.worker.launch");
        assert_eq!(params["agent"], "codex");
        assert_eq!(params["cwd"], "/repo/forktty");
        assert_eq!(params["args"], json!(["--model", "test"]));

        let (method, params) = build_socket_call_for_test(
            "team_worker_health",
            json!({"team_id": "team-1", "stale_after_ms": 123}),
        )
        .unwrap();
        assert_eq!(method, "team.worker.health");
        assert_eq!(params["stale_after_ms"], 123);

        let (method, params) = build_socket_call_for_test(
            "team_worker_nudge",
            json!({"team_id": "team-1", "worker_id": "worker-2", "text": "ping\r"}),
        )
        .unwrap();
        assert_eq!(method, "team.worker.nudge");
        assert_eq!(params["text"], "ping\r");

        let (method, params) = build_socket_call_for_test(
            "team_finish",
            json!({
                "team_id": "team-1",
                "dry_run": true,
                "close_workers": true,
                "force": true,
            }),
        )
        .unwrap();
        assert_eq!(method, "team.finish");
        assert_eq!(params["team_id"], "team-1");
        assert_eq!(params["dry_run"], true);
        assert_eq!(params["close_workers"], true);
        assert_eq!(params["force"], true);

        let (method, params) = build_socket_call_for_test(
            "team_worker_shutdown",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-2",
                "text": "stop",
                "submit": false,
                "close_surface": true,
            }),
        )
        .unwrap();
        assert_eq!(method, "team.worker.shutdown");
        assert_eq!(params["worker_id"], "worker-2");
        assert_eq!(params["text"], "stop");
        assert_eq!(params["submit"], false);
        assert_eq!(params["close_surface"], true);

        let (method, params) = build_socket_call_for_test(
            "team_task_upsert",
            json!({
                "team_id": "team-1",
                "task_id": "task-1",
                "depends_on": ["task-0", "task-base"]
            }),
        )
        .unwrap();
        assert_eq!(method, "team.task.upsert");
        assert_eq!(params["depends_on"], json!(["task-0", "task-base"]));

        let (method, params) = build_socket_call_for_test(
            "team_message_send",
            json!({
                "team_id": "team-1",
                "from": "leader",
                "body": "  continue\n",
                "to_worker_id": "worker-1"
            }),
        )
        .unwrap();
        assert_eq!(method, "team.message.send");
        assert_eq!(params["body"], "  continue\n");

        let (method, params) = build_socket_call_for_test(
            "team_message_dispatch",
            json!({
                "team_id": "team-1",
                "message_id": "msg-1",
                "worker_id": "worker-1",
                "submit": true
            }),
        )
        .unwrap();
        assert_eq!(method, "team.message.dispatch");
        assert_eq!(params["message_id"], "msg-1");
        assert_eq!(params["submit"], true);

        let (method, params) = build_socket_call_for_test(
            "team_inbox",
            json!({"team_id": "team-1", "worker_id": "worker-1", "include_delivered": true}),
        )
        .unwrap();
        assert_eq!(method, "team.inbox");
        assert_eq!(params["include_delivered"], true);
    }

    #[test]
    fn status_summary_tool_maps_to_socket_status_summary() {
        let (method, params) =
            build_socket_call_for_test("status_summary", json!({"workspace_id": "w1"})).unwrap();

        assert_eq!(method, "status.summary");
        assert_eq!(params["workspace_id"], "w1");
    }

    #[test]
    fn context_snapshot_tool_maps_to_socket_context_snapshot() {
        let (method, params) = build_socket_call_for_test(
            "context_snapshot",
            json!({
                "workspace_id": "w1",
                "tail_lines": 20,
                "tail_max_bytes": 4096,
                "include_team_details": true,
                "include_workflow_details": true,
                "include_feed_trace": true
            }),
        )
        .unwrap();

        assert_eq!(method, "context.snapshot");
        assert_eq!(params["workspace_id"], "w1");
        assert_eq!(params["tail_lines"], 20);
        assert_eq!(params["tail_max_bytes"], 4096);
        assert_eq!(params["include_team_details"], true);
        assert_eq!(params["include_workflow_details"], true);
        assert_eq!(params["include_feed_trace"], true);
    }

    #[test]
    fn identify_tool_maps_to_socket_system_identify() {
        let (method, params) = build_socket_call_for_test(
            "identify",
            json!({
                "workspace_id": "w1",
                "caller_workspace_id": "caller-w",
                "caller_surface_id": "caller-s"
            }),
        )
        .unwrap();

        assert_eq!(method, "system.identify");
        assert_eq!(params["workspace_id"], "w1");
        assert_eq!(params["caller_workspace_id"], "caller-w");
        assert_eq!(params["caller_surface_id"], "caller-s");
    }

    #[test]
    fn identify_tool_uses_env_only_as_caller_context_without_target_args() {
        with_env(
            &[
                ("FORKTTY_WORKSPACE_ID", Some("workspace-env")),
                ("FORKTTY_SURFACE_ID", Some("surface-env")),
            ],
            || {
                let (method, params) = build_socket_call_for_test("identify", json!({})).unwrap();

                assert_eq!(method, "system.identify");
                assert!(params.get("workspace_id").is_none());
                assert!(params.get("surface_id").is_none());
                assert_eq!(params["caller_workspace_id"], "workspace-env");
                assert_eq!(params["caller_surface_id"], "surface-env");
            },
        );
    }

    #[test]
    fn workflow_tools_map_to_socket_workflow_methods() {
        let (method, params) = build_socket_call_for_test(
            "workflow_list",
            json!({
                "workspace_id": "w1",
                "surface_id": "s1",
                "session_id": "sess1",
                "query": "goal",
                "limit": 5
            }),
        )
        .unwrap();
        assert_eq!(method, "workflow.list");
        assert_eq!(params["workspace_id"], "w1");
        assert_eq!(params["surface_id"], "s1");
        assert_eq!(params["session_id"], "sess1");
        assert_eq!(params["query"], "goal");
        assert_eq!(params["limit"], 5);

        let (method, params) =
            build_socket_call_for_test("workflow_get", json!({"workflow_id": "workflow-1"}))
                .unwrap();
        assert_eq!(method, "workflow.get");
        assert_eq!(params["workflow_id"], "workflow-1");

        let (method, params) = build_socket_call_for_test(
            "workflow_upsert",
            json!({
                "workflow_id": "workflow-1",
                "workspace_id": "w1",
                "agent": "codex",
                "session_id": "sess1",
                "mode": "review",
                "status": "running",
                "goal": "Review",
                "memory": "Keep context"
            }),
        )
        .unwrap();
        assert_eq!(method, "workflow.upsert");
        assert_eq!(params["workflow_id"], "workflow-1");
        assert_eq!(params["workspace_id"], "w1");
        assert_eq!(params["memory"], "Keep context");

        let steps = json!([{"id": "inspect", "title": "Inspect", "status": "done"}]);
        let (method, params) = build_socket_call_for_test(
            "workflow_plan_set",
            json!({"workflow_id": "workflow-1", "steps": steps}),
        )
        .unwrap();
        assert_eq!(method, "workflow.plan.set");
        assert_eq!(params["steps"][0]["id"], "inspect");

        let gates = json!([{
            "id": "fmt",
            "kind": "command",
            "label": "cargo fmt --all --check",
            "status": "passed",
            "summary": "formatting clean"
        }]);
        let (method, params) = build_socket_call_for_test(
            "workflow_loop_set",
            json!({
                "workflow_id": "workflow-1",
                "recipe": "review-fix-verify",
                "stage": "verify",
                "iteration": 2,
                "max_iterations": 3,
                "gates": gates
            }),
        )
        .unwrap();
        assert_eq!(method, "workflow.loop.set");
        assert_eq!(params["workflow_id"], "workflow-1");
        assert_eq!(params["recipe"], "review-fix-verify");
        assert_eq!(params["iteration"], 2);
        assert_eq!(params["gates"][0]["id"], "fmt");

        let (method, params) = build_socket_call_for_test(
            "workflow_evidence_add",
            json!({
                "workflow_id": "workflow-1",
                "evidence_id": "tests",
                "kind": "test",
                "title": "cargo test",
                "text": "  passed\n"
            }),
        )
        .unwrap();
        assert_eq!(method, "workflow.evidence.add");
        assert_eq!(params["evidence_id"], "tests");
        assert_eq!(params["text"], "  passed\n");

        // kind and title are required: the schema declares them required and the
        // socket server rejects requests that omit them, so reject locally too.
        assert!(build_socket_call_for_test(
            "workflow_evidence_add",
            json!({"workflow_id": "workflow-1", "text": "x"}),
        )
        .is_err());

        // At least one of text/path is required, matching the socket server.
        assert!(build_socket_call_for_test(
            "workflow_evidence_add",
            json!({"workflow_id": "workflow-1", "kind": "test", "title": "cargo test"}),
        )
        .is_err());

        let (method, params) = build_socket_call_for_test(
            "workflow_replay",
            json!({"workflow_id": "workflow-1", "since_seq": 2, "limit": 10}),
        )
        .unwrap();
        assert_eq!(method, "workflow.replay");
        assert_eq!(params["since_seq"], 2);
        assert_eq!(params["limit"], 10);
    }

    #[test]
    fn topology_tree_tool_maps_to_socket_topology_tree() {
        let (method, params) =
            build_socket_call_for_test("topology_tree", json!({"workspace_id": "w1"})).unwrap();

        assert_eq!(method, "topology.tree");
        assert_eq!(params["workspace_id"], "w1");
    }

    #[test]
    fn surface_read_tools_map_to_socket_capture_methods() {
        let (method, params) = build_socket_call_for_test(
            "surface_read_text",
            json!({"surface_id": "surface-1", "scope": "all", "max_bytes": 4096}),
        )
        .unwrap();
        assert_eq!(method, "surface.read_text");
        assert_eq!(params["surface_id"], "surface-1");
        assert_eq!(params["scope"], "all");
        assert_eq!(params["max_bytes"], 4096);

        let (method, params) = build_socket_call_for_test(
            "surface_capture_tail",
            json!({"surface_id": "surface-1", "lines": 40}),
        )
        .unwrap();
        assert_eq!(method, "surface.capture_tail");
        assert_eq!(params["surface_id"], "surface-1");
        assert_eq!(params["lines"], 40);
    }

    #[test]
    fn precondition_errors_carry_remedy_and_suggested_tool() {
        let (socket_path, requests_handle) = fake_socket(1, |request| {
            JsonRpcResponse::error(
                request["id"].clone(),
                "precondition_failed",
                "cwd is not inside the git repository of any open workspace",
            )
        });
        let input = br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"worktree_list","arguments":{"cwd":"/tmp"}}}
"#;
        let mut output = Vec::new();
        run_with_io(BufReader::new(&input[..]), &mut output, socket_path).unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        let structured = &response["result"]["structuredContent"];
        assert_eq!(structured["code"], "precondition_failed");
        assert_eq!(structured["suggested_tool"], "workspace_create");
        assert!(structured["remedy"].as_str().unwrap().contains("workspace"));
        assert_eq!(requests_handle.join().unwrap().len(), 1);
    }

    #[test]
    fn tools_call_workspace_create_forwards_working_dir() {
        let (socket_path, requests_handle) = fake_socket(1, |request| {
            assert_eq!(request["method"], "workspace.create");
            assert_eq!(request["params"]["working_dir"], "/tmp/repo");
            assert_eq!(request["params"]["name"], "scratch");
            JsonRpcResponse::ok(
                request["id"].clone(),
                json!({ "id": "workspace-9", "name": "scratch" }),
            )
        });
        let input = br#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"workspace_create","arguments":{"working_dir":"/tmp/repo","name":"scratch"}}}
"#;
        let mut output = Vec::new();
        run_with_io(BufReader::new(&input[..]), &mut output, socket_path).unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["result"]["structuredContent"]["id"], "workspace-9");
        assert_eq!(requests_handle.join().unwrap().len(), 1);
    }

    #[test]
    fn tools_call_workspace_create_requires_working_dir() {
        let input = br#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"workspace_create","arguments":{"name":"scratch"}}}
"#;
        let mut output = Vec::new();
        run_with_io(
            BufReader::new(&input[..]),
            &mut output,
            PathBuf::from("/run/user/1000/forktty.sock"),
        )
        .unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["result"]["isError"], true);
        assert!(response["result"]["structuredContent"]["message"]
            .as_str()
            .unwrap()
            .contains("requires working_dir"));
    }

    #[test]
    fn tools_call_wraps_array_results_in_object_structured_content() {
        let (socket_path, requests_handle) = fake_socket(1, |request| {
            assert_eq!(request["method"], "workspace.list");
            JsonRpcResponse::ok(
                request["id"].clone(),
                json!([{ "id": "workspace-1", "name": "main" }]),
            )
        });
        let input = br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"workspace_list","arguments":{}}}
"#;
        let mut output = Vec::new();
        run_with_io(BufReader::new(&input[..]), &mut output, socket_path).unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        let structured = &response["result"]["structuredContent"];
        assert!(
            structured.is_object(),
            "structuredContent must be a JSON object per the MCP spec: {structured}"
        );
        assert_eq!(structured["result"][0]["id"], "workspace-1");
        assert_eq!(requests_handle.join().unwrap().len(), 1);
    }

    #[test]
    fn tools_call_maps_socket_errors_to_tool_errors() {
        let (socket_path, requests_handle) = fake_socket(1, |request| {
            JsonRpcResponse::error(request["id"].clone(), "not_found", "Surface not found")
        });
        let input = br#"{"jsonrpc":"2.0","id":"focus","method":"tools/call","params":{"name":"surface_focus","arguments":{"surface_id":"missing"}}}
"#;
        let mut output = Vec::new();
        run_with_io(BufReader::new(&input[..]), &mut output, socket_path).unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["id"], "focus");
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(response["result"]["structuredContent"]["code"], "not_found");
        assert!(response["result"]["structuredContent"]["message"]
            .as_str()
            .unwrap()
            .contains("Surface not found"));
        assert_eq!(requests_handle.join().unwrap().len(), 1);
    }

    fn fake_socket(
        request_count: usize,
        responder: impl Fn(&Value) -> JsonRpcResponse + Send + 'static,
    ) -> (PathBuf, thread::JoinHandle<Vec<Value>>) {
        let dir = runtime_tempdir();
        let socket_path = dir.path().join("forktty.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let path = socket_path.clone();
        let handle = thread::spawn(move || {
            let _dir = dir;
            let mut requests = Vec::new();
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_socket_request(&mut stream);
                let response = responder(&request);
                serde_json::to_writer(&mut stream, &response).unwrap();
                stream.write_all(b"\n").unwrap();
                requests.push(request);
            }
            requests
        });
        (path, handle)
    }

    fn read_socket_request(stream: &mut UnixStream) -> Value {
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        serde_json::from_str(line.trim()).unwrap()
    }

    fn runtime_tempdir() -> tempfile::TempDir {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let fallback = std::env::current_dir().unwrap().join("target/test-runtime");
                fs::create_dir_all(&fallback).unwrap();
                fallback
            });
        tempfile::Builder::new()
            .prefix("forktty-mcp-")
            .tempdir_in(base)
            .unwrap()
    }

    fn annotation(name: &str) -> Value {
        tool_specs()
            .into_iter()
            .find(|tool| tool.name == name)
            .map(|tool| tool.annotations)
            .unwrap_or_else(|| panic!("missing MCP tool {name}"))
    }
}
