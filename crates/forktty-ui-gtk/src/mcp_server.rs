use crate::agent_guide;
use crate::socket_cli::{send_socket_request_with_timeout, CliResult};
use serde_json::{json, Map, Value};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
// Per the MCP spec, initialize echoes the client's requested version only when
// the server actually supports it; otherwise it answers with its own latest.
const MCP_SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
const MCP_SOCKET_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_MCP_MESSAGE_BYTES: usize = 1_048_576;

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

struct SocketCall {
    method: &'static str,
    params: Map<String, Value>,
}

#[derive(Debug)]
struct ToolCallError {
    code: &'static str,
    message: String,
    protocol_error: bool,
}

impl ToolCallError {
    fn validation(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_params",
            message: message.into(),
            protocol_error: false,
        }
    }

    fn protocol(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_params",
            message: message.into(),
            protocol_error: true,
        }
    }
}

fn tools_call_validation_error(err: ToolCallError) -> Value {
    tool_error_result(err.code, err.message)
}

fn tool_error_result(code: &str, message: impl Into<String>) -> Value {
    let message = message.into();
    let mut structured = json!({
        "code": code,
        "message": message,
    });
    if let Some((remedy, suggested_tool)) = error_recovery(code) {
        structured["remedy"] = json!(remedy);
        structured["suggested_tool"] = json!(suggested_tool);
    }
    json!({
        "isError": true,
        "content": [{
            "type": "text",
            "text": message,
        }],
        "structuredContent": structured,
    })
}

/// Machine-readable recovery for error codes with a known remedy. Only codes
/// whose recovery is real get fields — boilerplate on every error would
/// train agents to ignore them. `suggested_tool` must name a tool present in
/// tool_specs(); a test pins that.
fn error_recovery(code: &str) -> Option<(&'static str, &'static str)> {
    match code {
        "precondition_failed" => Some((
            "Open a ForkTTY workspace on the target repository first, then retry.",
            "workspace_create",
        )),
        _ => None,
    }
}

fn build_socket_call(name: &str, args: &Map<String, Value>) -> Result<SocketCall, ToolCallError> {
    let call = match name {
        "workspace_list" => {
            reject_unexpected(args, &[], name)?;
            SocketCall {
                method: "workspace.list",
                params: Map::new(),
            }
        }
        "workspace_create" => {
            reject_unexpected(args, &["name", "working_dir"], name)?;
            let mut params = Map::new();
            let working_dir = optional_non_blank(args, "working_dir")?.ok_or_else(|| {
                ToolCallError::validation("workspace_create requires working_dir")
            })?;
            params.insert("working_dir".to_string(), Value::String(working_dir));
            if let Some(workspace_name) = optional_non_blank(args, "name")? {
                params.insert("name".to_string(), Value::String(workspace_name));
            }
            SocketCall {
                method: "workspace.create",
                params,
            }
        }
        "surface_list" => {
            reject_unexpected(
                args,
                &["workspace_id", "workspace_name", "worktree_name"],
                name,
            )?;
            SocketCall {
                method: "surface.list",
                params: workspace_target_params(args, true)?,
            }
        }
        "topology_tree" => {
            reject_unexpected(
                args,
                &["workspace_id", "workspace_name", "worktree_name"],
                name,
            )?;
            SocketCall {
                method: "topology.tree",
                params: workspace_target_params(args, true)?,
            }
        }
        "agent_list" => {
            reject_unexpected(
                args,
                &["workspace_id", "workspace_name", "worktree_name"],
                name,
            )?;
            SocketCall {
                method: "agent.list",
                params: workspace_target_params(args, true)?,
            }
        }
        "agent_health" => {
            reject_unexpected(
                args,
                &["workspace_id", "workspace_name", "worktree_name"],
                name,
            )?;
            SocketCall {
                method: "agent.health",
                params: workspace_target_params(args, true)?,
            }
        }
        "agent_reclaim_plan" => {
            reject_unexpected(
                args,
                &[
                    "workspace_id",
                    "workspace_name",
                    "worktree_name",
                    "min_idle_ms",
                ],
                name,
            )?;
            let mut params = workspace_target_params(args, true)?;
            if let Some(min_idle_ms) = optional_u64(args, "min_idle_ms")? {
                params.insert("min_idle_ms".to_string(), Value::Number(min_idle_ms.into()));
            }
            SocketCall {
                method: "agent.reclaim.plan",
                params,
            }
        }
        "agent_hibernate" => {
            reject_unexpected(args, &["surface_id", "min_idle_ms"], name)?;
            let mut params = Map::new();
            params.insert(
                "surface_id".to_string(),
                Value::String(required_non_empty_string(args, "surface_id")?),
            );
            if let Some(min_idle_ms) = optional_u64(args, "min_idle_ms")? {
                params.insert("min_idle_ms".to_string(), Value::Number(min_idle_ms.into()));
            }
            SocketCall {
                method: "agent.hibernate",
                params,
            }
        }
        "agent_reclaim" => {
            reject_unexpected(
                args,
                &[
                    "workspace_id",
                    "workspace_name",
                    "worktree_name",
                    "min_idle_ms",
                    "limit",
                ],
                name,
            )?;
            let mut params = workspace_target_params(args, true)?;
            if let Some(min_idle_ms) = optional_u64(args, "min_idle_ms")? {
                params.insert("min_idle_ms".to_string(), Value::Number(min_idle_ms.into()));
            }
            if let Some(limit) = optional_u64(args, "limit")? {
                params.insert("limit".to_string(), Value::Number(limit.into()));
            }
            SocketCall {
                method: "agent.reclaim",
                params,
            }
        }
        "agent_resume" => {
            reject_unexpected(args, &["surface_id"], name)?;
            let mut params = Map::new();
            params.insert(
                "surface_id".to_string(),
                Value::String(required_non_empty_string(args, "surface_id")?),
            );
            SocketCall {
                method: "agent.resume",
                params,
            }
        }
        "team_list" => {
            reject_unexpected(
                args,
                &[
                    "workspace_id",
                    "workspace_name",
                    "worktree_name",
                    "status",
                    "query",
                    "limit",
                ],
                name,
            )?;
            let mut params = workspace_target_params(args, true)?;
            insert_optional_non_blank_param(args, &mut params, "status")?;
            insert_optional_non_blank_param(args, &mut params, "query")?;
            insert_optional_u64_param(args, &mut params, "limit")?;
            SocketCall {
                method: "team.list",
                params,
            }
        }
        "team_get" => {
            reject_unexpected(args, &["team_id"], name)?;
            SocketCall {
                method: "team.get",
                params: map_from_pairs([("team_id", required_non_blank(args, "team_id")?)]),
            }
        }
        "team_upsert" => {
            reject_unexpected(
                args,
                &[
                    "team_id",
                    "workspace_id",
                    "workspace_name",
                    "worktree_name",
                    "leader_surface_id",
                    "name",
                    "status",
                    "goal",
                ],
                name,
            )?;
            let mut params = workspace_target_params(args, true)?;
            params.insert(
                "team_id".to_string(),
                Value::String(required_non_blank(args, "team_id")?),
            );
            insert_optional_non_blank_param(args, &mut params, "leader_surface_id")?;
            insert_optional_non_blank_param(args, &mut params, "name")?;
            insert_optional_non_blank_param(args, &mut params, "status")?;
            insert_optional_string_param(args, &mut params, "goal")?;
            SocketCall {
                method: "team.upsert",
                params,
            }
        }
        "team_worker_upsert" => {
            reject_unexpected(
                args,
                &[
                    "team_id",
                    "worker_id",
                    "role",
                    "agent",
                    "surface_id",
                    "worktree_name",
                    "status",
                    "assigned_task_id",
                ],
                name,
            )?;
            let mut params = map_from_pairs([
                ("team_id", required_non_blank(args, "team_id")?),
                ("worker_id", required_non_blank(args, "worker_id")?),
            ]);
            insert_optional_non_blank_param(args, &mut params, "role")?;
            insert_optional_non_blank_param(args, &mut params, "agent")?;
            insert_optional_non_blank_param(args, &mut params, "surface_id")?;
            insert_optional_non_blank_param(args, &mut params, "worktree_name")?;
            insert_optional_non_blank_param(args, &mut params, "status")?;
            insert_optional_non_blank_param(args, &mut params, "assigned_task_id")?;
            SocketCall {
                method: "team.worker.upsert",
                params,
            }
        }
        "team_worker_heartbeat" => {
            reject_unexpected(
                args,
                &["team_id", "worker_id", "status", "assigned_task_id"],
                name,
            )?;
            let mut params = map_from_pairs([
                ("team_id", required_non_blank(args, "team_id")?),
                ("worker_id", required_non_blank(args, "worker_id")?),
            ]);
            insert_optional_non_blank_param(args, &mut params, "status")?;
            insert_optional_non_blank_param(args, &mut params, "assigned_task_id")?;
            SocketCall {
                method: "team.worker.heartbeat",
                params,
            }
        }
        "team_worker_launch" => {
            reject_unexpected(
                args,
                &[
                    "team_id",
                    "worker_id",
                    "agent",
                    "role",
                    "assigned_task_id",
                    "worktree_name",
                    "args",
                ],
                name,
            )?;
            let mut params = map_from_pairs([
                ("team_id", required_non_blank(args, "team_id")?),
                ("worker_id", required_non_blank(args, "worker_id")?),
                ("agent", required_non_blank(args, "agent")?),
            ]);
            insert_optional_non_blank_param(args, &mut params, "role")?;
            insert_optional_non_blank_param(args, &mut params, "assigned_task_id")?;
            insert_optional_non_blank_param(args, &mut params, "worktree_name")?;
            if let Some(extra_args) = optional_string_array(args, "args")? {
                params.insert(
                    "args".to_string(),
                    Value::Array(extra_args.into_iter().map(Value::String).collect()),
                );
            }
            SocketCall {
                method: "team.worker.launch",
                params,
            }
        }
        "team_worker_health" => {
            reject_unexpected(args, &["team_id", "stale_after_ms"], name)?;
            let mut params = map_from_pairs([("team_id", required_non_blank(args, "team_id")?)]);
            insert_optional_u64_param(args, &mut params, "stale_after_ms")?;
            SocketCall {
                method: "team.worker.health",
                params,
            }
        }
        "team_worker_nudge" => {
            reject_unexpected(args, &["team_id", "worker_id", "text"], name)?;
            let mut params = map_from_pairs([
                ("team_id", required_non_blank(args, "team_id")?),
                ("worker_id", required_non_blank(args, "worker_id")?),
            ]);
            insert_optional_string_param(args, &mut params, "text")?;
            SocketCall {
                method: "team.worker.nudge",
                params,
            }
        }
        "team_worker_shutdown" => {
            reject_unexpected(args, &["team_id", "worker_id", "text"], name)?;
            let mut params = map_from_pairs([
                ("team_id", required_non_blank(args, "team_id")?),
                ("worker_id", required_non_blank(args, "worker_id")?),
            ]);
            insert_optional_string_param(args, &mut params, "text")?;
            SocketCall {
                method: "team.worker.shutdown",
                params,
            }
        }
        "team_task_upsert" => {
            reject_unexpected(
                args,
                &[
                    "team_id",
                    "task_id",
                    "title",
                    "status",
                    "detail",
                    "depends_on",
                    "assigned_worker_id",
                ],
                name,
            )?;
            let mut params = map_from_pairs([
                ("team_id", required_non_blank(args, "team_id")?),
                ("task_id", required_non_blank(args, "task_id")?),
            ]);
            insert_optional_non_blank_param(args, &mut params, "title")?;
            insert_optional_non_blank_param(args, &mut params, "status")?;
            insert_optional_string_param(args, &mut params, "detail")?;
            if let Some(depends_on) = optional_string_array(args, "depends_on")? {
                params.insert(
                    "depends_on".to_string(),
                    Value::Array(depends_on.into_iter().map(Value::String).collect()),
                );
            }
            insert_optional_non_blank_param(args, &mut params, "assigned_worker_id")?;
            SocketCall {
                method: "team.task.upsert",
                params,
            }
        }
        "team_message_send" => {
            reject_unexpected(
                args,
                &[
                    "team_id",
                    "message_id",
                    "from",
                    "to_worker_id",
                    "task_id",
                    "body",
                ],
                name,
            )?;
            let mut params = map_from_pairs([
                ("team_id", required_non_blank(args, "team_id")?),
                ("from", required_non_blank(args, "from")?),
                ("body", required_non_empty_string(args, "body")?),
            ]);
            insert_optional_non_blank_param(args, &mut params, "message_id")?;
            insert_optional_non_blank_param(args, &mut params, "to_worker_id")?;
            insert_optional_non_blank_param(args, &mut params, "task_id")?;
            SocketCall {
                method: "team.message.send",
                params,
            }
        }
        "team_message_dispatch" => {
            reject_unexpected(args, &["team_id", "message_id", "worker_id"], name)?;
            let mut params = map_from_pairs([
                ("team_id", required_non_blank(args, "team_id")?),
                ("message_id", required_non_blank(args, "message_id")?),
            ]);
            insert_optional_non_blank_param(args, &mut params, "worker_id")?;
            SocketCall {
                method: "team.message.dispatch",
                params,
            }
        }
        "team_message_ack" => {
            reject_unexpected(args, &["team_id", "message_id", "worker_id"], name)?;
            let mut params = map_from_pairs([
                ("team_id", required_non_blank(args, "team_id")?),
                ("message_id", required_non_blank(args, "message_id")?),
            ]);
            insert_optional_non_blank_param(args, &mut params, "worker_id")?;
            SocketCall {
                method: "team.message.ack",
                params,
            }
        }
        "team_inbox" => {
            reject_unexpected(
                args,
                &["team_id", "worker_id", "include_delivered", "limit"],
                name,
            )?;
            let mut params = map_from_pairs([("team_id", required_non_blank(args, "team_id")?)]);
            insert_optional_non_blank_param(args, &mut params, "worker_id")?;
            insert_optional_bool_param(args, &mut params, "include_delivered")?;
            insert_optional_u64_param(args, &mut params, "limit")?;
            SocketCall {
                method: "team.inbox",
                params,
            }
        }
        "team_summary" => {
            reject_unexpected(args, &["team_id"], name)?;
            SocketCall {
                method: "team.summary",
                params: map_from_pairs([("team_id", required_non_blank(args, "team_id")?)]),
            }
        }
        "team_events" => {
            reject_unexpected(args, &["team_id", "since_seq", "limit"], name)?;
            let mut params = Map::new();
            insert_optional_non_blank_param(args, &mut params, "team_id")?;
            insert_optional_u64_param(args, &mut params, "since_seq")?;
            insert_optional_u64_param(args, &mut params, "limit")?;
            SocketCall {
                method: "team.events",
                params,
            }
        }
        "status_summary" => {
            reject_unexpected(
                args,
                &["workspace_id", "workspace_name", "worktree_name"],
                name,
            )?;
            SocketCall {
                method: "status.summary",
                params: workspace_target_params(args, true)?,
            }
        }
        "workflow_list" => {
            reject_unexpected(
                args,
                &[
                    "workspace_id",
                    "workspace_name",
                    "worktree_name",
                    "surface_id",
                    "session_id",
                    "query",
                    "limit",
                ],
                name,
            )?;
            let mut params = workspace_target_params(args, true)?;
            if let Some(surface_id) = optional_non_blank(args, "surface_id")? {
                params.insert("surface_id".to_string(), Value::String(surface_id));
            }
            if let Some(session_id) = optional_non_blank(args, "session_id")? {
                params.insert("session_id".to_string(), Value::String(session_id));
            }
            if let Some(query) = optional_non_blank(args, "query")? {
                params.insert("query".to_string(), Value::String(query));
            }
            if let Some(limit) = optional_u64(args, "limit")? {
                params.insert("limit".to_string(), Value::Number(limit.into()));
            }
            SocketCall {
                method: "workflow.list",
                params,
            }
        }
        "workflow_get" => {
            reject_unexpected(args, &["workflow_id"], name)?;
            SocketCall {
                method: "workflow.get",
                params: map_from_pairs([("workflow_id", required_non_blank(args, "workflow_id")?)]),
            }
        }
        "workflow_upsert" => {
            reject_unexpected(
                args,
                &[
                    "workflow_id",
                    "workspace_id",
                    "workspace_name",
                    "worktree_name",
                    "surface_id",
                    "agent",
                    "session_id",
                    "mode",
                    "status",
                    "goal",
                    "memory",
                ],
                name,
            )?;
            let mut params = workspace_target_params(args, false)?;
            for key in [
                "workflow_id",
                "surface_id",
                "agent",
                "session_id",
                "mode",
                "status",
                "goal",
                "memory",
            ] {
                if let Some(value) = optional_non_blank(args, key)? {
                    params.insert(key.to_string(), Value::String(value));
                }
            }
            SocketCall {
                method: "workflow.upsert",
                params,
            }
        }
        "workflow_plan_set" => {
            reject_unexpected(args, &["workflow_id", "steps"], name)?;
            let mut params =
                map_from_pairs([("workflow_id", required_non_blank(args, "workflow_id")?)]);
            let steps = args
                .get("steps")
                .cloned()
                .ok_or_else(|| ToolCallError::validation("workflow_plan_set requires steps"))?;
            if !steps.is_array() {
                return Err(ToolCallError::validation("steps must be an array"));
            }
            params.insert("steps".to_string(), steps);
            SocketCall {
                method: "workflow.plan.set",
                params,
            }
        }
        "workflow_evidence_add" => {
            reject_unexpected(
                args,
                &[
                    "workflow_id",
                    "evidence_id",
                    "kind",
                    "title",
                    "text",
                    "path",
                ],
                name,
            )?;
            let mut params =
                map_from_pairs([("workflow_id", required_non_blank(args, "workflow_id")?)]);
            for key in ["evidence_id", "kind", "title", "path"] {
                if let Some(value) = optional_non_blank(args, key)? {
                    params.insert(key.to_string(), Value::String(value));
                }
            }
            if let Some(text) = optional_string(args, "text")? {
                if text.trim().is_empty() {
                    return Err(ToolCallError::validation("text must not be empty"));
                }
                params.insert("text".to_string(), Value::String(text));
            }
            SocketCall {
                method: "workflow.evidence.add",
                params,
            }
        }
        "workflow_replay" => {
            reject_unexpected(args, &["workflow_id", "query", "since_seq", "limit"], name)?;
            let mut params = Map::new();
            if let Some(workflow_id) = optional_non_blank(args, "workflow_id")? {
                params.insert("workflow_id".to_string(), Value::String(workflow_id));
            }
            if let Some(query) = optional_non_blank(args, "query")? {
                params.insert("query".to_string(), Value::String(query));
            }
            if let Some(since_seq) = optional_u64(args, "since_seq")? {
                params.insert("since_seq".to_string(), Value::Number(since_seq.into()));
            }
            if let Some(limit) = optional_u64(args, "limit")? {
                params.insert("limit".to_string(), Value::Number(limit.into()));
            }
            SocketCall {
                method: "workflow.replay",
                params,
            }
        }
        "surface_split" => {
            reject_unexpected(args, &["surface_id", "axis"], name)?;
            let mut params = Map::new();
            let surface_id = optional_non_blank(args, "surface_id")?
                .or_else(|| trimmed_env("FORKTTY_SURFACE_ID"))
                .ok_or_else(|| {
                    ToolCallError::validation(
                        "surface_split requires surface_id or FORKTTY_SURFACE_ID",
                    )
                })?;
            params.insert("surface_id".to_string(), Value::String(surface_id));
            if let Some(axis) = optional_enum(args, "axis", &["horizontal", "vertical"])? {
                params.insert("axis".to_string(), Value::String(axis));
            }
            SocketCall {
                method: "surface.split",
                params,
            }
        }
        "surface_send_text" => {
            reject_unexpected(args, &["surface_id", "text"], name)?;
            let text = required_non_empty_string(args, "text")?;
            let surface_id = optional_non_blank(args, "surface_id")?
                .or_else(|| trimmed_env("FORKTTY_SURFACE_ID"))
                .ok_or_else(|| {
                    ToolCallError::validation(
                        "surface_send_text requires surface_id or FORKTTY_SURFACE_ID",
                    )
                })?;
            SocketCall {
                method: "surface.send_text",
                params: map_from_pairs([("surface_id", surface_id), ("text", text)]),
            }
        }
        "surface_read_text" => {
            reject_unexpected(args, &["surface_id", "scope", "max_bytes"], name)?;
            let surface_id = optional_non_blank(args, "surface_id")?
                .or_else(|| trimmed_env("FORKTTY_SURFACE_ID"))
                .ok_or_else(|| {
                    ToolCallError::validation(
                        "surface_read_text requires surface_id or FORKTTY_SURFACE_ID",
                    )
                })?;
            let mut params = map_from_pairs([("surface_id", surface_id)]);
            if let Some(scope) = optional_enum(args, "scope", &["visible", "all"])? {
                params.insert("scope".to_string(), Value::String(scope));
            }
            if let Some(max_bytes) = optional_u64(args, "max_bytes")? {
                params.insert("max_bytes".to_string(), Value::Number(max_bytes.into()));
            }
            SocketCall {
                method: "surface.read_text",
                params,
            }
        }
        "surface_capture_tail" => {
            reject_unexpected(args, &["surface_id", "lines", "max_bytes"], name)?;
            let surface_id = optional_non_blank(args, "surface_id")?
                .or_else(|| trimmed_env("FORKTTY_SURFACE_ID"))
                .ok_or_else(|| {
                    ToolCallError::validation(
                        "surface_capture_tail requires surface_id or FORKTTY_SURFACE_ID",
                    )
                })?;
            let mut params = map_from_pairs([("surface_id", surface_id)]);
            if let Some(lines) = optional_u64(args, "lines")? {
                params.insert("lines".to_string(), Value::Number(lines.into()));
            }
            if let Some(max_bytes) = optional_u64(args, "max_bytes")? {
                params.insert("max_bytes".to_string(), Value::Number(max_bytes.into()));
            }
            SocketCall {
                method: "surface.capture_tail",
                params,
            }
        }
        "surface_focus" => {
            reject_unexpected(args, &["surface_id"], name)?;
            let surface_id = optional_non_blank(args, "surface_id")?
                .or_else(|| trimmed_env("FORKTTY_SURFACE_ID"))
                .ok_or_else(|| {
                    ToolCallError::validation(
                        "surface_focus requires surface_id or FORKTTY_SURFACE_ID",
                    )
                })?;
            SocketCall {
                method: "surface.focus",
                params: map_from_pairs([("surface_id", surface_id)]),
            }
        }
        "worktree_list" => {
            reject_unexpected(args, &["cwd"], name)?;
            SocketCall {
                method: "worktree.list",
                params: map_from_pairs([("cwd", required_non_blank(args, "cwd")?)]),
            }
        }
        "worktree_status" => {
            reject_unexpected(args, &["path", "cwd"], name)?;
            let path = optional_non_blank(args, "path")?;
            let cwd = optional_non_blank(args, "cwd")?;
            if path.is_some() && cwd.is_some() {
                return Err(ToolCallError::validation(
                    "worktree_status cannot combine path and cwd",
                ));
            }
            let path = path
                .or(cwd)
                .ok_or_else(|| ToolCallError::validation("worktree_status requires path or cwd"))?;
            SocketCall {
                method: "worktree.status",
                params: map_from_pairs([("path", path)]),
            }
        }
        "worktree_create" => worktree_named_call(name, args, "worktree.create")?,
        "worktree_attach" => worktree_named_call(name, args, "worktree.attach")?,
        "worktree_remove" => worktree_named_call(name, args, "worktree.remove")?,
        "worktree_merge" => worktree_named_call(name, args, "worktree.merge")?,
        "notification_create" => {
            reject_unexpected(
                args,
                &[
                    "workspace_id",
                    "workspace_name",
                    "worktree_name",
                    "surface_id",
                    "title",
                    "body",
                    "kind",
                ],
                name,
            )?;
            let mut params = workspace_and_surface_target_params(args)?;
            if let Some(title) = optional_non_blank(args, "title")? {
                params.insert("title".to_string(), Value::String(title));
            }
            if let Some(body) = optional_string(args, "body")? {
                params.insert("body".to_string(), Value::String(body));
            }
            if let Some(kind) = optional_enum(args, "kind", &["info", "prompt", "error", "custom"])?
            {
                params.insert("kind".to_string(), Value::String(kind));
            }
            SocketCall {
                method: "notification.create",
                params,
            }
        }
        "status_set" => {
            reject_unexpected(
                args,
                &[
                    "workspace_id",
                    "workspace_name",
                    "worktree_name",
                    "surface_id",
                    "key",
                    "label",
                    "value",
                    "color",
                ],
                name,
            )?;
            let key = required_non_blank(args, "key")?;
            let value = required_non_blank(args, "value")?;
            let label = optional_non_blank(args, "label")?.unwrap_or_else(|| key.clone());
            let mut params = workspace_and_surface_target_params(args)?;
            params.insert("key".to_string(), Value::String(key));
            params.insert("label".to_string(), Value::String(label));
            params.insert("value".to_string(), Value::String(value));
            if let Some(color) = optional_non_blank(args, "color")? {
                if !is_supported_status_color(&color) {
                    return Err(ToolCallError::validation(format!(
                        "Unsupported status color: {color}"
                    )));
                }
                params.insert("color".to_string(), Value::String(color));
            }
            SocketCall {
                method: "metadata.set_status",
                params,
            }
        }
        _ => return Err(ToolCallError::protocol(format!("Unknown tool: {name}"))),
    };
    Ok(call)
}

fn tools_call_result_with_validation(
    name: &str,
    arguments: &Map<String, Value>,
    socket_path: &Path,
) -> Result<Value, ProtocolError> {
    let call = match build_socket_call(name, arguments) {
        Ok(call) => call,
        Err(err) if err.protocol_error => {
            return Err(ProtocolError {
                code: -32602,
                message: err.message,
            });
        }
        Err(err) => return Ok(tools_call_validation_error(err)),
    };
    match send_socket_request_with_timeout(
        socket_path,
        call.method,
        Value::Object(call.params),
        MCP_SOCKET_TIMEOUT,
    ) {
        Ok(result) => {
            let text = success_text(name, &result);
            // The MCP spec requires structuredContent to be a JSON object;
            // socket list methods return bare arrays, so wrap non-objects
            // (strict clients reject the whole tool result otherwise).
            let structured = if result.is_object() {
                result
            } else {
                json!({ "result": result })
            };
            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": text,
                }],
                "structuredContent": structured,
            }))
        }
        Err(err) => Ok(tool_error_result(
            err.code.as_deref().unwrap_or("socket_error"),
            sanitize_tool_error_message(&err.message),
        )),
    }
}

fn worktree_named_call(
    name: &str,
    args: &Map<String, Value>,
    method: &'static str,
) -> Result<SocketCall, ToolCallError> {
    reject_unexpected(args, &["cwd", "name"], name)?;
    Ok(SocketCall {
        method,
        params: map_from_pairs([
            ("cwd", required_non_blank(args, "cwd")?),
            ("name", required_non_blank(args, "name")?),
        ]),
    })
}

fn workspace_target_params(
    args: &Map<String, Value>,
    default_workspace: bool,
) -> Result<Map<String, Value>, ToolCallError> {
    let workspace_id = optional_non_blank(args, "workspace_id")?;
    let workspace_name = optional_non_blank(args, "workspace_name")?;
    let worktree_name = optional_non_blank(args, "worktree_name")?;
    let selector_count = [&workspace_id, &workspace_name, &worktree_name]
        .into_iter()
        .filter(|value| value.is_some())
        .count();
    if selector_count > 1 {
        return Err(ToolCallError::validation(
            "cannot combine workspace_id, workspace_name, and worktree_name",
        ));
    }
    let mut params = Map::new();
    if let Some(value) = workspace_id {
        params.insert("workspace_id".to_string(), Value::String(value));
    } else if let Some(value) = workspace_name {
        params.insert("workspace_name".to_string(), Value::String(value));
    } else if let Some(value) = worktree_name {
        params.insert("worktree_name".to_string(), Value::String(value));
    } else if default_workspace {
        if let Some(value) = trimmed_env("FORKTTY_WORKSPACE_ID") {
            params.insert("workspace_id".to_string(), Value::String(value));
        }
    }
    Ok(params)
}

fn workspace_and_surface_target_params(
    args: &Map<String, Value>,
) -> Result<Map<String, Value>, ToolCallError> {
    let mut params = workspace_target_params(args, false)?;
    if let Some(surface_id) = optional_non_blank(args, "surface_id")? {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    }
    if params.is_empty() {
        if let Some(workspace_id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
            params.insert("workspace_id".to_string(), Value::String(workspace_id));
        }
        if let Some(surface_id) = trimmed_env("FORKTTY_SURFACE_ID") {
            params.insert("surface_id".to_string(), Value::String(surface_id));
        }
    }
    Ok(params)
}

fn map_from_pairs<const N: usize>(pairs: [(&str, String); N]) -> Map<String, Value> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_string(), Value::String(value)))
        .collect()
}

fn insert_optional_non_blank_param(
    args: &Map<String, Value>,
    params: &mut Map<String, Value>,
    key: &'static str,
) -> Result<(), ToolCallError> {
    if let Some(value) = optional_non_blank(args, key)? {
        params.insert(key.to_string(), Value::String(value));
    }
    Ok(())
}

fn insert_optional_string_param(
    args: &Map<String, Value>,
    params: &mut Map<String, Value>,
    key: &'static str,
) -> Result<(), ToolCallError> {
    if let Some(value) = optional_string(args, key)? {
        params.insert(key.to_string(), Value::String(value));
    }
    Ok(())
}

fn insert_optional_u64_param(
    args: &Map<String, Value>,
    params: &mut Map<String, Value>,
    key: &'static str,
) -> Result<(), ToolCallError> {
    if let Some(value) = optional_u64(args, key)? {
        params.insert(key.to_string(), Value::Number(value.into()));
    }
    Ok(())
}

fn insert_optional_bool_param(
    args: &Map<String, Value>,
    params: &mut Map<String, Value>,
    key: &'static str,
) -> Result<(), ToolCallError> {
    if let Some(value) = optional_bool(args, key)? {
        params.insert(key.to_string(), Value::Bool(value));
    }
    Ok(())
}

fn reject_unexpected(
    args: &Map<String, Value>,
    allowed: &[&str],
    tool: &str,
) -> Result<(), ToolCallError> {
    for key in args.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(ToolCallError::validation(format!(
                "{tool}: unexpected argument {key}"
            )));
        }
    }
    Ok(())
}

fn required_non_blank(
    args: &Map<String, Value>,
    key: &'static str,
) -> Result<String, ToolCallError> {
    optional_non_blank(args, key)?
        .ok_or_else(|| ToolCallError::validation(format!("{key} is required")))
}

fn required_non_empty_string(
    args: &Map<String, Value>,
    key: &'static str,
) -> Result<String, ToolCallError> {
    let value = optional_string(args, key)?
        .ok_or_else(|| ToolCallError::validation(format!("{key} is required")))?;
    if value.is_empty() {
        return Err(ToolCallError::validation(format!(
            "{key} must not be empty"
        )));
    }
    Ok(value)
}

fn optional_non_blank(
    args: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<String>, ToolCallError> {
    let Some(value) = optional_string(args, key)? else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ToolCallError::validation(format!(
            "{key} must not be empty"
        )));
    }
    Ok(Some(trimmed.to_string()))
}

fn optional_string(
    args: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<String>, ToolCallError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(ToolCallError::validation(format!("{key} must be a string"))),
    }
}

fn optional_u64(
    args: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<u64>, ToolCallError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| ToolCallError::validation(format!("{key} must be an unsigned integer"))),
        Some(_) => Err(ToolCallError::validation(format!(
            "{key} must be an unsigned integer"
        ))),
    }
}

fn optional_bool(
    args: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<bool>, ToolCallError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(ToolCallError::validation(format!(
            "{key} must be a boolean"
        ))),
    }
}

fn optional_string_array(
    args: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<Vec<String>>, ToolCallError> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Value::Array(items) = value else {
        return Err(ToolCallError::validation(format!(
            "{key} must be an array of strings"
        )));
    };
    let mut values = Vec::with_capacity(items.len());
    for item in items {
        let Some(value) = item.as_str() else {
            return Err(ToolCallError::validation(format!(
                "{key} must be an array of strings"
            )));
        };
        let value = value.trim();
        if value.is_empty() {
            return Err(ToolCallError::validation(format!(
                "{key} entries must not be empty"
            )));
        }
        values.push(value.to_string());
    }
    Ok(Some(values))
}

fn optional_enum(
    args: &Map<String, Value>,
    key: &'static str,
    values: &[&str],
) -> Result<Option<String>, ToolCallError> {
    let Some(value) = optional_non_blank(args, key)? else {
        return Ok(None);
    };
    if values.contains(&value.as_str()) {
        Ok(Some(value))
    } else {
        Err(ToolCallError::validation(format!(
            "{key} must be one of {}",
            values.join(", ")
        )))
    }
}

fn trimmed_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn is_supported_status_color(color: &str) -> bool {
    matches!(color, "green" | "yellow" | "red" | "blue" | "muted") || is_hex_status_color(color)
}

fn is_hex_status_color(color: &str) -> bool {
    let Some(hex) = color.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn success_text(name: &str, result: &Value) -> String {
    match name {
        "workspace_list" => "Listed ForkTTY workspaces.".to_string(),
        "surface_list" => "Listed ForkTTY surfaces.".to_string(),
        "topology_tree" => "Built ForkTTY topology tree.".to_string(),
        "agent_list" => "Listed ForkTTY agent sessions.".to_string(),
        "agent_health" => "Checked ForkTTY agent session readiness.".to_string(),
        "agent_reclaim_plan" => "Planned ForkTTY agent session reclaim candidates.".to_string(),
        "agent_hibernate" => "Hibernated ForkTTY agent session.".to_string(),
        "agent_reclaim" => "Reclaimed ForkTTY idle agent sessions.".to_string(),
        "agent_resume" => "Resumed ForkTTY agent session in a new tab.".to_string(),
        "team_list" => "Listed ForkTTY teams.".to_string(),
        "team_get" => "Read ForkTTY team state.".to_string(),
        "team_upsert" => "Updated ForkTTY team state.".to_string(),
        "team_worker_upsert" => "Updated ForkTTY team worker state.".to_string(),
        "team_worker_heartbeat" => "Recorded ForkTTY team worker heartbeat.".to_string(),
        "team_worker_launch" => "Launched ForkTTY team worker pane.".to_string(),
        "team_worker_health" => "Read ForkTTY team worker health.".to_string(),
        "team_worker_nudge" => "Nudged ForkTTY team worker pane.".to_string(),
        "team_worker_shutdown" => "Requested ForkTTY team worker shutdown.".to_string(),
        "team_task_upsert" => "Updated ForkTTY team task state.".to_string(),
        "team_message_send" => "Queued ForkTTY team message.".to_string(),
        "team_message_dispatch" => "Dispatched ForkTTY team message to a worker pane.".to_string(),
        "team_message_ack" => "Acknowledged ForkTTY team message.".to_string(),
        "team_inbox" => "Read ForkTTY team inbox.".to_string(),
        "team_summary" => "Summarized ForkTTY team state.".to_string(),
        "team_events" => "Listed ForkTTY team events.".to_string(),
        "status_summary" => "Built ForkTTY status summary.".to_string(),
        "workflow_list" => "Listed ForkTTY workflows.".to_string(),
        "workflow_get" => "Read ForkTTY workflow state.".to_string(),
        "workflow_upsert" => "Updated ForkTTY workflow state.".to_string(),
        "workflow_plan_set" => "Updated ForkTTY workflow plan.".to_string(),
        "workflow_evidence_add" => "Added ForkTTY workflow evidence.".to_string(),
        "workflow_replay" => "Replayed ForkTTY workflow events.".to_string(),
        "surface_split" => format!(
            "Created ForkTTY surface {}.",
            result
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("(unknown)")
        ),
        "surface_send_text" => "Sent text to the ForkTTY surface.".to_string(),
        "surface_read_text" => "Read text from the ForkTTY surface.".to_string(),
        "surface_capture_tail" => "Captured text tail from the ForkTTY surface.".to_string(),
        "surface_focus" => "Focused the ForkTTY surface.".to_string(),
        "worktree_list" => "Listed git worktrees.".to_string(),
        "worktree_status" => format!(
            "Worktree status: {}.",
            result
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ),
        "worktree_create" | "worktree_attach" => format!(
            "Opened ForkTTY worktree workspace {}.",
            result
                .get("name")
                .or_else(|| result.get("branch"))
                .and_then(Value::as_str)
                .unwrap_or("(unknown)")
        ),
        "worktree_remove" => "Removed the ForkTTY worktree.".to_string(),
        "worktree_merge" => "Merged the ForkTTY worktree.".to_string(),
        "notification_create" => "Created a ForkTTY notification.".to_string(),
        "status_set" => "Updated ForkTTY status metadata.".to_string(),
        _ => "ForkTTY tool call completed.".to_string(),
    }
}

fn sanitize_tool_error_message(message: &str) -> String {
    message
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>()
}

struct ToolSpec {
    name: &'static str,
    description: &'static str,
    input_schema: Value,
    annotations: Value,
}

// MCP tool annotations (2025-03-26 spec): UX hints for clients, not a
// security boundary. Most ForkTTY tools act on the local instance only, but
// terminal-input tools can drive shells that touch files, networks, or other
// external systems, so they must opt into open-world/destructive hints.
fn read_only_annotations() -> Value {
    json!({ "readOnlyHint": true, "openWorldHint": false })
}

fn mutating_annotations(destructive: bool, idempotent: bool) -> Value {
    mutating_annotations_with_open_world(destructive, idempotent, false)
}

fn mutating_annotations_with_open_world(
    destructive: bool,
    idempotent: bool,
    open_world: bool,
) -> Value {
    json!({
        "readOnlyHint": false,
        "destructiveHint": destructive,
        "idempotentHint": idempotent,
        "openWorldHint": open_world,
    })
}

fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "workspace_list",
            annotations: read_only_annotations(),
            description: "List ForkTTY workspaces, including active workspace, branch, worktree, and focused surface information. Start here before targeting panes.",
            input_schema: object_schema(&[], json!({})),
        },
        ToolSpec {
            name: "workspace_create",
            annotations: mutating_annotations(false, false),
            description: "Open a new ForkTTY workspace on a directory. Use this to satisfy the worktree tools' open-workspace precondition before worktree_create.",
            input_schema: object_schema(
                &["working_dir"],
                json!({
                    "working_dir": string_prop("Directory the workspace opens in (the repo root for worktree work)."),
                    "name": string_prop("Workspace name; defaults to \"workspace\"."),
                }),
            ),
        },
        ToolSpec {
            name: "surface_list",
            annotations: read_only_annotations(),
            description: "List panes/surfaces in a workspace. Defaults to FORKTTY_WORKSPACE_ID when launched from a ForkTTY pane; omit targeting to inspect that pane's workspace.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                }),
            ),
        },
        ToolSpec {
            name: "topology_tree",
            annotations: read_only_annotations(),
            description: "Return a read-only ForkTTY workspace/pane/surface tree with surfaces nested under each workspace. Use before targeting a different pane.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                }),
            ),
        },
        ToolSpec {
            name: "agent_list",
            annotations: read_only_annotations(),
            description: "List ForkTTY surfaces with persisted agent session ids. Use before planning manual resume or status/HUD work.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                }),
            ),
        },
        ToolSpec {
            name: "agent_health",
            annotations: read_only_annotations(),
            description: "Check whether persisted ForkTTY agent sessions have a safe provider resume command and provider executable on PATH.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                }),
            ),
        },
        ToolSpec {
            name: "agent_reclaim_plan",
            annotations: read_only_annotations(),
            description: "Plan which persisted idle agent sessions are safe reclaim candidates without suspending or closing anything.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                    "min_idle_ms": integer_prop("Minimum idle age in milliseconds before a session can be a reclaim candidate."),
                }),
            ),
        },
        ToolSpec {
            name: "agent_hibernate",
            annotations: mutating_annotations(false, false),
            description: "Hibernate one idle, resumable ForkTTY agent session by closing its terminal process and marking it suspended. Prefer agent_reclaim_plan first.",
            input_schema: object_schema(
                &["surface_id"],
                json!({
                    "surface_id": string_prop("Surface id with an idle persisted agent session."),
                    "min_idle_ms": integer_prop("Optional minimum idle age in milliseconds before hibernating."),
                }),
            ),
        },
        ToolSpec {
            name: "agent_reclaim",
            annotations: mutating_annotations(false, false),
            description: "Apply the reclaim policy to idle, resumable ForkTTY agent sessions and hibernate matching candidates.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                    "min_idle_ms": integer_prop("Minimum idle age in milliseconds before a session can be reclaimed."),
                    "limit": integer_prop("Maximum number of candidate sessions to hibernate, capped by the socket."),
                }),
            ),
        },
        ToolSpec {
            name: "agent_resume",
            annotations: mutating_annotations(false, false),
            description: "Resume a persisted agent session from a source surface into a new tab using ForkTTY's argv-only provider command builder. Prefer agent_health first and continue only when the row is ready.",
            input_schema: object_schema(
                &["surface_id"],
                json!({
                    "surface_id": string_prop("Source surface id with a persisted agent session from agent_list or status_summary."),
                }),
            ),
        },
        ToolSpec {
            name: "team_list",
            annotations: read_only_annotations(),
            description: "List ForkTTY team orchestration records for the current or selected workspace. Use before updating workers, tasks, or mailbox messages.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                    "status": string_prop("Optional team status filter."),
                    "query": string_prop("Optional substring query across team id, name, and goal."),
                    "limit": integer_prop("Maximum teams to return."),
                }),
            ),
        },
        ToolSpec {
            name: "team_get",
            annotations: read_only_annotations(),
            description: "Read one ForkTTY team orchestration record including workers, tasks, and mailbox messages.",
            input_schema: object_schema(
                &["team_id"],
                json!({
                    "team_id": string_prop("Team id from team_list or team_upsert."),
                }),
            ),
        },
        ToolSpec {
            name: "team_upsert",
            annotations: mutating_annotations(false, true),
            description: "Create or update a ForkTTY team orchestration record. This stores leader metadata only; it does not open UI panes.",
            input_schema: object_schema(
                &["team_id"],
                json!({
                    "team_id": string_prop("Stable team id."),
                    "workspace_id": string_prop("Workspace id to associate with the team; defaults from FORKTTY_WORKSPACE_ID when present."),
                    "workspace_name": string_prop("Workspace name to associate with the team."),
                    "worktree_name": string_prop("Worktree name to associate with the team."),
                    "leader_surface_id": string_prop("Surface id for the leader pane."),
                    "name": string_prop("Human team name."),
                    "status": string_prop("Team status, for example active, paused, or done."),
                    "goal": string_prop("Team goal or brief."),
                }),
            ),
        },
        ToolSpec {
            name: "team_worker_upsert",
            annotations: mutating_annotations(false, true),
            description: "Create or update a worker record for a ForkTTY team. Surface/worktree fields are references; this does not spawn panes.",
            input_schema: object_schema(
                &["team_id", "worker_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "worker_id": string_prop("Stable worker id."),
                    "role": string_prop("Worker role."),
                    "agent": string_prop("Agent/provider name."),
                    "surface_id": string_prop("Surface id used by this worker."),
                    "worktree_name": string_prop("Worktree name assigned to this worker."),
                    "status": string_prop("Worker status, for example idle, running, busy, or blocked."),
                    "assigned_task_id": string_prop("Task id currently assigned to this worker."),
                }),
            ),
        },
        ToolSpec {
            name: "team_worker_heartbeat",
            annotations: mutating_annotations(false, true),
            description: "Record a worker heartbeat/status update for a ForkTTY team.",
            input_schema: object_schema(
                &["team_id", "worker_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "worker_id": string_prop("Worker id."),
                    "status": string_prop("Current worker status."),
                    "assigned_task_id": string_prop("Current assigned task id."),
                }),
            ),
        },
        ToolSpec {
            name: "team_worker_launch",
            annotations: mutating_annotations(false, false),
            description: "Launch a provider worker in a new ForkTTY tab and attach it to a team worker record. Supported agents are codex, claude, gemini, opencode, and antigravity.",
            input_schema: object_schema(
                &["team_id", "worker_id", "agent"],
                json!({
                    "team_id": string_prop("Team id."),
                    "worker_id": string_prop("Worker id."),
                    "agent": string_prop("Provider to launch: codex, claude, gemini, opencode, or antigravity."),
                    "role": string_prop("Worker role."),
                    "assigned_task_id": string_prop("Task id currently assigned to this worker."),
                    "worktree_name": string_prop("Worktree name assigned to this worker."),
                    "args": string_array_prop("Extra argv entries appended after the provider executable."),
                }),
            ),
        },
        ToolSpec {
            name: "team_worker_health",
            annotations: read_only_annotations(),
            description: "Read per-worker team health including stale heartbeat, surface presence, nudge, launch, and shutdown-request timestamps.",
            input_schema: object_schema(
                &["team_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "stale_after_ms": integer_prop("Heartbeat age after which running workers are reported stale."),
                }),
            ),
        },
        ToolSpec {
            name: "team_worker_nudge",
            annotations: mutating_annotations(false, false),
            description: "Send a nudge message to a team worker's attached terminal pane and record the nudge timestamp after delivery succeeds.",
            input_schema: object_schema(
                &["team_id", "worker_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "worker_id": string_prop("Worker id."),
                    "text": string_prop("Optional exact text to send; defaults to a heartbeat request."),
                }),
            ),
        },
        ToolSpec {
            name: "team_worker_shutdown",
            annotations: mutating_annotations(false, false),
            description: "Request safe shutdown by sending text to a team worker's attached terminal pane and marking the worker shutdown_requested after delivery succeeds.",
            input_schema: object_schema(
                &["team_id", "worker_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "worker_id": string_prop("Worker id."),
                    "text": string_prop("Optional exact shutdown request text."),
                }),
            ),
        },
        ToolSpec {
            name: "team_task_upsert",
            annotations: mutating_annotations(false, true),
            description: "Create or update a ForkTTY team task. Dependencies must form a DAG.",
            input_schema: object_schema(
                &["team_id", "task_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "task_id": string_prop("Stable task id."),
                    "title": string_prop("Task title."),
                    "status": string_prop("Task status, for example open, running, done, blocked, or cancelled."),
                    "detail": string_prop("Task detail or notes."),
                    "depends_on": string_array_prop("Task ids this task depends on."),
                    "assigned_worker_id": string_prop("Worker id assigned to this task."),
                }),
            ),
        },
        ToolSpec {
            name: "team_message_send",
            annotations: mutating_annotations(false, false),
            description: "Queue a mailbox message for a ForkTTY team worker or team-wide coordination.",
            input_schema: object_schema(
                &["team_id", "from", "body"],
                json!({
                    "team_id": string_prop("Team id."),
                    "message_id": string_prop("Optional stable message id; ForkTTY generates one when omitted."),
                    "from": string_prop("Sender id, usually leader or a worker id."),
                    "to_worker_id": string_prop("Target worker id. Omit for team-wide messages."),
                    "task_id": string_prop("Related task id."),
                    "body": string_prop("Message body. Whitespace is preserved."),
                }),
            ),
        },
        ToolSpec {
            name: "team_message_dispatch",
            annotations: mutating_annotations(false, false),
            description: "Send a queued ForkTTY team message to a worker terminal pane and acknowledge it only after the terminal accepts the text.",
            input_schema: object_schema(
                &["team_id", "message_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "message_id": string_prop("Message id."),
                    "worker_id": string_prop("Required when dispatching a team-wide message."),
                }),
            ),
        },
        ToolSpec {
            name: "team_message_ack",
            annotations: mutating_annotations(false, true),
            description: "Acknowledge a ForkTTY team mailbox message.",
            input_schema: object_schema(
                &["team_id", "message_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "message_id": string_prop("Message id."),
                    "worker_id": string_prop("Optional worker id guard for worker-targeted acknowledgements."),
                }),
            ),
        },
        ToolSpec {
            name: "team_inbox",
            annotations: read_only_annotations(),
            description: "Read pending mailbox messages for a ForkTTY team or worker.",
            input_schema: object_schema(
                &["team_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "worker_id": string_prop("Worker id to filter messages."),
                    "include_delivered": boolean_prop("Include already acknowledged messages."),
                    "limit": integer_prop("Maximum messages to return."),
                }),
            ),
        },
        ToolSpec {
            name: "team_summary",
            annotations: read_only_annotations(),
            description: "Return aggregate counts for a ForkTTY team: workers, active workers, tasks, pending messages, and last event.",
            input_schema: object_schema(
                &["team_id"],
                json!({
                    "team_id": string_prop("Team id."),
                }),
            ),
        },
        ToolSpec {
            name: "team_events",
            annotations: read_only_annotations(),
            description: "List ForkTTY team orchestration events for polling or audit.",
            input_schema: object_schema(
                &[],
                json!({
                    "team_id": string_prop("Optional team id filter."),
                    "since_seq": integer_prop("Only return events with seq greater than this value."),
                    "limit": integer_prop("Maximum events to return."),
                }),
            ),
        },
        ToolSpec {
            name: "status_summary",
            annotations: read_only_annotations(),
            description: "Return a compact workspace summary with persisted agent sessions, status entries, and progress entries for statusline/HUD integrations.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to summarize."),
                    "workspace_name": string_prop("Workspace name to summarize."),
                    "worktree_name": string_prop("Worktree name to summarize."),
                }),
            ),
        },
        ToolSpec {
            name: "workflow_list",
            annotations: read_only_annotations(),
            description: "List durable ForkTTY workflow states for goals, mode/session memory, plan steps, and evidence. Defaults to FORKTTY_WORKSPACE_ID when available.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to filter."),
                    "workspace_name": string_prop("Workspace name to filter."),
                    "worktree_name": string_prop("Worktree name to filter."),
                    "surface_id": string_prop("Surface id to filter."),
                    "session_id": string_prop("Provider session id to filter."),
                    "query": string_prop("Case-insensitive search across workflow goal, memory, plan, and evidence."),
                    "limit": integer_prop("Maximum workflow rows to return; socket enforces its upper bound."),
                }),
            ),
        },
        ToolSpec {
            name: "workflow_get",
            annotations: read_only_annotations(),
            description: "Read one durable ForkTTY workflow state by workflow id.",
            input_schema: object_schema(
                &["workflow_id"],
                json!({
                    "workflow_id": string_prop("Workflow id from workflow_list or workflow_upsert."),
                }),
            ),
        },
        ToolSpec {
            name: "workflow_upsert",
            annotations: mutating_annotations(false, false),
            description: "Create or update durable ForkTTY workflow state for a workspace, surface, provider session, mode, goal, and compaction-resistant memory.",
            input_schema: object_schema(
                &[],
                json!({
                    "workflow_id": string_prop("Optional explicit workflow id. If omitted, ForkTTY derives one from session_id, surface_id, or workspace_id."),
                    "workspace_id": string_prop("Workspace id to bind."),
                    "workspace_name": string_prop("Workspace name to bind."),
                    "worktree_name": string_prop("Worktree name to bind."),
                    "surface_id": string_prop("Surface id to bind."),
                    "agent": string_prop("Provider name such as codex or claude."),
                    "session_id": string_prop("Provider session id."),
                    "mode": string_prop("Workflow mode, for example review, plan, implementation, or default."),
                    "status": string_prop("Workflow status, for example running, blocked, done, or active."),
                    "goal": string_prop("Current workflow goal."),
                    "memory": string_prop("Durable project/session memory for compaction recovery."),
                }),
            ),
        },
        ToolSpec {
            name: "workflow_plan_set",
            annotations: mutating_annotations(false, true),
            description: "Replace a workflow's durable plan steps. Each step object must include id, title, and status; detail is optional.",
            input_schema: object_schema(
                &["workflow_id", "steps"],
                json!({
                    "workflow_id": string_prop("Workflow id to update."),
                    "steps": {
                        "type": "array",
                        "description": "Array of {id,title,status,detail?} plan step objects.",
                        "items": { "type": "object" },
                    },
                }),
            ),
        },
        ToolSpec {
            name: "workflow_evidence_add",
            annotations: mutating_annotations(false, false),
            description: "Append a bounded evidence artifact to a workflow. Provide text or a path reference; ForkTTY stores metadata and optional text, not arbitrary unbounded files.",
            input_schema: object_schema(
                &["workflow_id", "kind", "title"],
                json!({
                    "workflow_id": string_prop("Workflow id to update."),
                    "evidence_id": string_prop("Optional evidence id; ForkTTY derives one when omitted."),
                    "kind": string_prop("Evidence kind such as test, diff, note, log, or decision."),
                    "title": string_prop("Evidence title."),
                    "text": string_prop("Bounded evidence text."),
                    "path": string_prop("Optional path reference for an artifact already on disk."),
                }),
            ),
        },
        ToolSpec {
            name: "workflow_replay",
            annotations: read_only_annotations(),
            description: "Replay durable ForkTTY workflow events for session search and recovery.",
            input_schema: object_schema(
                &[],
                json!({
                    "workflow_id": string_prop("Workflow id to filter."),
                    "query": string_prop("Case-insensitive event search."),
                    "since_seq": integer_prop("Only return events after this sequence number."),
                    "limit": integer_prop("Maximum event rows to return; socket enforces its upper bound."),
                }),
            ),
        },
        ToolSpec {
            name: "surface_split",
            annotations: mutating_annotations(false, false),
            description: "Split a ForkTTY surface to create another agent-ready pane. Defaults surface_id from FORKTTY_SURFACE_ID when present.",
            input_schema: object_schema(
                &[],
                json!({
                    "surface_id": string_prop("Surface id to split; defaults from FORKTTY_SURFACE_ID."),
                    "axis": enum_prop("Split direction.", &["horizontal", "vertical"]),
                }),
            ),
        },
        ToolSpec {
            name: "surface_send_text",
            annotations: mutating_annotations_with_open_world(true, false, true),
            description: "Send literal text to a ForkTTY terminal surface. Text may execute shell commands or interact with external systems; use this to drive a pane after inspecting surface_list; surface_id defaults from FORKTTY_SURFACE_ID.",
            input_schema: object_schema(
                &["text"],
                json!({
                    "surface_id": string_prop("Target terminal surface id; defaults from FORKTTY_SURFACE_ID."),
                    "text": string_prop("Literal text to send to the terminal."),
                }),
            ),
        },
        ToolSpec {
            name: "surface_read_text",
            annotations: read_only_annotations(),
            description: "Read text from a ForkTTY terminal surface. Defaults to visible screen text and FORKTTY_SURFACE_ID; use scope=all only when a bounded full scrollback read is needed.",
            input_schema: object_schema(
                &[],
                json!({
                    "surface_id": string_prop("Target terminal surface id; defaults from FORKTTY_SURFACE_ID."),
                    "scope": enum_prop("Text range to read.", &["visible", "all"]),
                    "max_bytes": integer_prop("Maximum UTF-8 bytes to return; socket enforces its upper bound."),
                }),
            ),
        },
        ToolSpec {
            name: "surface_capture_tail",
            annotations: read_only_annotations(),
            description: "Capture the last N lines of a ForkTTY terminal surface, including scrollback. Defaults surface_id from FORKTTY_SURFACE_ID.",
            input_schema: object_schema(
                &[],
                json!({
                    "surface_id": string_prop("Target terminal surface id; defaults from FORKTTY_SURFACE_ID."),
                    "lines": integer_prop("Number of trailing lines to capture; defaults to 80."),
                    "max_bytes": integer_prop("Maximum UTF-8 bytes to return; socket enforces its upper bound."),
                }),
            ),
        },
        ToolSpec {
            name: "surface_focus",
            annotations: mutating_annotations(false, true),
            description: "Focus a ForkTTY surface in the UI. Defaults surface_id from FORKTTY_SURFACE_ID when present.",
            input_schema: object_schema(
                &[],
                json!({
                    "surface_id": string_prop("Surface id to focus; defaults from FORKTTY_SURFACE_ID."),
                }),
            ),
        },
        ToolSpec {
            name: "worktree_list",
            annotations: read_only_annotations(),
            description: "List git worktrees for a repository path so agents can see existing parallel work. Requires an open ForkTTY workspace on the target repository (see workspace_list).",
            input_schema: object_schema(
                &["cwd"],
                json!({
                    "cwd": string_prop("Path inside the git repository."),
                }),
            ),
        },
        ToolSpec {
            name: "worktree_status",
            annotations: read_only_annotations(),
            description: "Report whether a worktree is clean, dirty, or otherwise blocked before attach/remove/merge operations. Requires an open ForkTTY workspace on the target repository (see workspace_list).",
            input_schema: object_schema(
                &[],
                json!({
                    "path": string_prop("Path to the worktree to inspect."),
                    "cwd": string_prop("Alternative path inside the worktree."),
                }),
            ),
        },
        ToolSpec {
            name: "worktree_create",
            annotations: mutating_annotations(false, false),
            description: "Create an isolated git worktree + ForkTTY workspace for parallel agent work. Pass a branch/worktree name and repo cwd. Requires an open ForkTTY workspace on the target repository (see workspace_list).",
            input_schema: worktree_named_schema(),
        },
        ToolSpec {
            name: "worktree_attach",
            annotations: mutating_annotations(false, false),
            description: "Attach an existing branch/worktree to a ForkTTY workspace so another agent can work there. Requires an open ForkTTY workspace on the target repository (see workspace_list).",
            input_schema: worktree_named_schema(),
        },
        ToolSpec {
            name: "worktree_remove",
            annotations: mutating_annotations(true, false),
            description: "Remove a ForkTTY-managed worktree after checking worktree_status; closes its workspace when present. Requires an open ForkTTY workspace on the target repository (see workspace_list).",
            input_schema: worktree_named_schema(),
        },
        ToolSpec {
            name: "worktree_merge",
            annotations: mutating_annotations(false, false),
            description: "Merge a completed worktree branch back into the repository. Use worktree_status first to avoid merging dirty work. Requires an open ForkTTY workspace on the target repository (see workspace_list).",
            input_schema: worktree_named_schema(),
        },
        ToolSpec {
            name: "notification_create",
            annotations: mutating_annotations(false, false),
            description: "Create a ForkTTY notification for the user. Defaults workspace_id/surface_id from the ForkTTY pane environment when no target is supplied.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Target workspace id."),
                    "workspace_name": string_prop("Target workspace name."),
                    "worktree_name": string_prop("Target worktree name."),
                    "surface_id": string_prop("Target surface id."),
                    "title": string_prop("Notification title; defaults to ForkTTY."),
                    "body": string_prop("Notification body."),
                    "kind": enum_prop("Notification kind.", &["info", "prompt", "error", "custom"]),
                }),
            ),
        },
        ToolSpec {
            name: "status_set",
            annotations: mutating_annotations(false, true),
            description: "Set ForkTTY sidebar status metadata for an agent or task. Defaults workspace_id/surface_id from the ForkTTY pane environment when no target is supplied.",
            input_schema: object_schema(
                &["key", "value"],
                json!({
                    "workspace_id": string_prop("Target workspace id."),
                    "workspace_name": string_prop("Target workspace name."),
                    "worktree_name": string_prop("Target worktree name."),
                    "surface_id": string_prop("Target surface id."),
                    "key": string_prop("Stable status key, for example agent:codex."),
                    "label": string_prop("Human label; defaults to key."),
                    "value": string_prop("Status value to display."),
                    "color": string_prop("green, yellow, red, blue, muted, or a CSS hex color."),
                }),
            ),
        },
    ]
}

fn worktree_named_schema() -> Value {
    object_schema(
        &["cwd", "name"],
        json!({
            "cwd": string_prop("Path inside the git repository."),
            "name": string_prop("Branch or worktree name."),
        }),
    )
}

fn object_schema(required: &[&str], properties: Value) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn string_prop(description: &str) -> Value {
    json!({
        "type": "string",
        "description": description,
    })
}

fn integer_prop(description: &str) -> Value {
    json!({
        "type": "integer",
        "minimum": 0,
        "description": description,
    })
}

fn boolean_prop(description: &str) -> Value {
    json!({
        "type": "boolean",
        "description": description,
    })
}

fn string_array_prop(description: &str) -> Value {
    json!({
        "type": "array",
        "items": { "type": "string" },
        "description": description,
    })
}

fn enum_prop(description: &str, values: &[&str]) -> Value {
    json!({
        "type": "string",
        "description": description,
        "enum": values,
    })
}

#[cfg(test)]
pub(crate) fn build_socket_call_for_test(
    name: &str,
    arguments: Value,
) -> Result<(String, Value), String> {
    let args = arguments
        .as_object()
        .ok_or_else(|| "arguments must be an object".to_string())?;
    let call = build_socket_call(name, args).map_err(|err| err.message)?;
    Ok((call.method.to_string(), Value::Object(call.params)))
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(annotation("surface_read_text")["readOnlyHint"], true);
        assert_eq!(annotation("surface_capture_tail")["readOnlyHint"], true);
        assert_eq!(annotation("worktree_remove")["destructiveHint"], true);
        assert_eq!(annotation("status_set")["idempotentHint"], true);
        assert_eq!(annotation("surface_send_text")["destructiveHint"], true);
        assert_eq!(annotation("surface_send_text")["openWorldHint"], true);
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
        assert!(resource_text.contains(
            "Use ForkTTY tools when the task involves panes, workspaces, agent sessions, workflow memory, team orchestration state, worktrees, status, terminal read/capture, or sending text to another surface."
        ));
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
                "args": ["--model", "test"]
            }),
        )
        .unwrap();
        assert_eq!(method, "team.worker.launch");
        assert_eq!(params["agent"], "codex");
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
            "team_worker_shutdown",
            json!({"team_id": "team-1", "worker_id": "worker-2"}),
        )
        .unwrap();
        assert_eq!(method, "team.worker.shutdown");
        assert_eq!(params["worker_id"], "worker-2");

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
            json!({"team_id": "team-1", "message_id": "msg-1", "worker_id": "worker-1"}),
        )
        .unwrap();
        assert_eq!(method, "team.message.dispatch");
        assert_eq!(params["message_id"], "msg-1");

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
}
