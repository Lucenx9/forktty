//! MCP tool-call validation, socket dispatch mapping, and result formatting.

use super::tool_params::{
    insert_optional_bool_param, insert_optional_non_blank_param, insert_optional_string_param,
    insert_optional_u64_param, map_from_pairs, optional_bool, optional_enum, optional_non_blank,
    optional_string, optional_string_array, optional_u64, reject_unexpected, required_non_blank,
    required_non_empty_string, ToolCallError,
};
use super::ProtocolError;
use crate::socket_cli::send_socket_request_with_timeout;
use forktty_core::protocol_limits;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::time::Duration;

const MCP_SOCKET_TIMEOUT: Duration = protocol_limits::OFFICIAL_SOCKET_TIMEOUT;

struct SocketCall {
    method: &'static str,
    params: Map<String, Value>,
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
pub(super) fn error_recovery(code: &str) -> Option<(&'static str, &'static str)> {
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
        "context_snapshot" => {
            reject_unexpected(
                args,
                &[
                    "workspace_id",
                    "workspace_name",
                    "worktree_name",
                    "surface_id",
                    "tail_lines",
                    "tail_max_bytes",
                    "include_team_details",
                    "include_workflow_details",
                    "include_feed_trace",
                ],
                name,
            )?;
            let mut params = workspace_target_params(args, true)?;
            insert_optional_non_blank_param(args, &mut params, "surface_id")?;
            insert_optional_u64_param(args, &mut params, "tail_lines")?;
            insert_optional_u64_param(args, &mut params, "tail_max_bytes")?;
            insert_optional_bool_param(args, &mut params, "include_team_details")?;
            insert_optional_bool_param(args, &mut params, "include_workflow_details")?;
            insert_optional_bool_param(args, &mut params, "include_feed_trace")?;
            SocketCall {
                method: "context.snapshot",
                params,
            }
        }
        "task_strategy_plan" => {
            reject_unexpected(
                args,
                &[
                    "goal",
                    "strategy",
                    "task_kind",
                    "router_profile",
                    "last_known_good",
                    "harness_signals",
                    "repo_dirty",
                    "workspace_id",
                    "workspace_name",
                    "worktree_name",
                    "surface_id",
                    "cwd",
                    "parallel",
                    "review",
                    "user_visible",
                ],
                name,
            )?;
            let mut params = workspace_target_params(args, true)?;
            params.insert(
                "goal".to_string(),
                Value::String(required_non_blank(args, "goal")?),
            );
            insert_optional_non_blank_param(args, &mut params, "strategy")?;
            insert_optional_non_blank_param(args, &mut params, "task_kind")?;
            insert_optional_non_blank_param(args, &mut params, "router_profile")?;
            insert_optional_object_param(args, &mut params, "last_known_good")?;
            insert_optional_object_param(args, &mut params, "harness_signals")?;
            if let Some(surface_id) = optional_non_blank(args, "surface_id")? {
                if !has_workspace_selector_arg(args) {
                    params.remove("workspace_id");
                }
                params.insert("surface_id".to_string(), Value::String(surface_id));
            }
            insert_optional_non_blank_param(args, &mut params, "cwd")?;
            insert_optional_bool_param(args, &mut params, "repo_dirty")?;
            insert_optional_renamed_bool_param(
                args,
                &mut params,
                "parallel",
                "user_requested_parallelism",
            )?;
            insert_optional_renamed_bool_param(
                args,
                &mut params,
                "review",
                "user_requested_review",
            )?;
            insert_optional_renamed_bool_param(
                args,
                &mut params,
                "user_visible",
                "likely_user_visible_change",
            )?;
            SocketCall {
                method: "task.strategy.plan",
                params,
            }
        }
        "task_strategy_apply" => {
            reject_unexpected(
                args,
                &[
                    "run_id",
                    "goal",
                    "plan",
                    "approved",
                    "approval_id",
                    "request_approval",
                    "workspace_id",
                    "workspace_name",
                    "worktree_name",
                    "cwd",
                    "leader_surface_id",
                    "surface_id",
                    "workflow_id",
                    "team_id",
                    "submit",
                ],
                name,
            )?;
            let mut params = workspace_target_params(args, true)?;
            params.insert(
                "run_id".to_string(),
                Value::String(required_non_blank(args, "run_id")?),
            );
            params.insert(
                "goal".to_string(),
                Value::String(required_non_blank(args, "goal")?),
            );
            let Some(plan) = args.get("plan") else {
                return Err(ToolCallError::validation("plan is required"));
            };
            if !plan.is_object() {
                return Err(ToolCallError::validation("plan must be an object"));
            }
            params.insert("plan".to_string(), plan.clone());
            if let Some(approved) = optional_string_array(args, "approved")? {
                params.insert(
                    "approved".to_string(),
                    Value::Array(approved.into_iter().map(Value::String).collect()),
                );
            }
            insert_optional_non_blank_param(args, &mut params, "approval_id")?;
            insert_optional_bool_param(args, &mut params, "request_approval")?;
            insert_optional_non_blank_param(args, &mut params, "cwd")?;
            if let Some(surface_id) = optional_non_blank(args, "leader_surface_id")? {
                if !has_workspace_selector_arg(args) {
                    params.remove("workspace_id");
                }
                params.insert("leader_surface_id".to_string(), Value::String(surface_id));
            } else if let Some(surface_id) = optional_non_blank(args, "surface_id")? {
                if !has_workspace_selector_arg(args) {
                    params.remove("workspace_id");
                }
                params.insert("surface_id".to_string(), Value::String(surface_id));
            } else if let Some(surface_id) = trimmed_env("FORKTTY_SURFACE_ID") {
                params.insert("leader_surface_id".to_string(), Value::String(surface_id));
            }
            insert_optional_non_blank_param(args, &mut params, "workflow_id")?;
            insert_optional_non_blank_param(args, &mut params, "team_id")?;
            insert_optional_bool_param(args, &mut params, "submit")?;
            SocketCall {
                method: "task.strategy.apply",
                params,
            }
        }
        "orchestration_cleanup" => {
            reject_unexpected(args, &["workspace_id", "apply", "dry_run"], name)?;
            let mut params = Map::new();
            insert_optional_non_blank_param(args, &mut params, "workspace_id")?;
            insert_optional_bool_param(args, &mut params, "apply")?;
            insert_optional_bool_param(args, &mut params, "dry_run")?;
            SocketCall {
                method: "orchestration.cleanup",
                params,
            }
        }
        "identify" => {
            reject_unexpected(
                args,
                &[
                    "workspace_id",
                    "workspace_name",
                    "worktree_name",
                    "surface_id",
                    "caller_workspace_id",
                    "caller_surface_id",
                ],
                name,
            )?;
            let mut params = workspace_target_params(args, false)?;
            insert_optional_non_blank_param(args, &mut params, "surface_id")?;
            if let Some(caller_workspace_id) = optional_non_blank(args, "caller_workspace_id")?
                .or_else(|| trimmed_env("FORKTTY_WORKSPACE_ID"))
            {
                params.insert(
                    "caller_workspace_id".to_string(),
                    Value::String(caller_workspace_id),
                );
            }
            if let Some(caller_surface_id) = optional_non_blank(args, "caller_surface_id")?
                .or_else(|| trimmed_env("FORKTTY_SURFACE_ID"))
            {
                params.insert(
                    "caller_surface_id".to_string(),
                    Value::String(caller_surface_id),
                );
            }
            SocketCall {
                method: "system.identify",
                params,
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
        "remote_list" => {
            reject_unexpected(
                args,
                &["workspace_id", "workspace_name", "worktree_name"],
                name,
            )?;
            SocketCall {
                method: "remote.list",
                params: workspace_target_params(args, true)?,
            }
        }
        "remote_status" => {
            reject_unexpected(
                args,
                &[
                    "surface_id",
                    "workspace_id",
                    "workspace_name",
                    "worktree_name",
                ],
                name,
            )?;
            let mut params = workspace_target_params(args, true)?;
            if let Some(surface_id) = optional_non_blank(args, "surface_id")? {
                params.insert("surface_id".to_string(), Value::String(surface_id));
            }
            SocketCall {
                method: "remote.status",
                params,
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
            let mut params = workspace_target_params(args, false)?;
            let leader_surface_id = optional_non_blank(args, "leader_surface_id")?;
            if let Some(surface_id) = leader_surface_id {
                params.insert("leader_surface_id".to_string(), Value::String(surface_id));
            } else if params.is_empty() {
                if let Some(surface_id) = trimmed_env("FORKTTY_SURFACE_ID") {
                    params.insert("leader_surface_id".to_string(), Value::String(surface_id));
                } else if let Some(workspace_id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
                    params.insert("workspace_id".to_string(), Value::String(workspace_id));
                }
            }
            params.insert(
                "team_id".to_string(),
                Value::String(required_non_blank(args, "team_id")?),
            );
            insert_optional_non_blank_param(args, &mut params, "name")?;
            insert_optional_non_blank_param(args, &mut params, "status")?;
            insert_optional_string_param(args, &mut params, "goal")?;
            SocketCall {
                method: "team.upsert",
                params,
            }
        }
        "team_finish" => {
            reject_unexpected(
                args,
                &["team_id", "dry_run", "close_workers", "force"],
                name,
            )?;
            let mut params = map_from_pairs([("team_id", required_non_blank(args, "team_id")?)]);
            insert_optional_bool_param(args, &mut params, "dry_run")?;
            insert_optional_bool_param(args, &mut params, "close_workers")?;
            insert_optional_bool_param(args, &mut params, "force")?;
            SocketCall {
                method: "team.finish",
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
                    "cwd",
                    "args",
                ],
                name,
            )?;
            let mut params = map_from_pairs([
                ("team_id", required_non_blank(args, "team_id")?),
                ("worker_id", required_non_blank(args, "worker_id")?),
            ]);
            insert_optional_non_blank_param(args, &mut params, "agent")?;
            insert_optional_non_blank_param(args, &mut params, "role")?;
            insert_optional_non_blank_param(args, &mut params, "assigned_task_id")?;
            insert_optional_non_blank_param(args, &mut params, "worktree_name")?;
            insert_optional_non_blank_param(args, &mut params, "cwd")?;
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
            reject_unexpected(
                args,
                &["team_id", "worker_id", "text", "submit", "close_surface"],
                name,
            )?;
            let mut params = map_from_pairs([
                ("team_id", required_non_blank(args, "team_id")?),
                ("worker_id", required_non_blank(args, "worker_id")?),
            ]);
            insert_optional_string_param(args, &mut params, "text")?;
            insert_optional_bool_param(args, &mut params, "submit")?;
            insert_optional_bool_param(args, &mut params, "close_surface")?;
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
            reject_unexpected(
                args,
                &["team_id", "message_id", "worker_id", "submit"],
                name,
            )?;
            let mut params = map_from_pairs([
                ("team_id", required_non_blank(args, "team_id")?),
                ("message_id", required_non_blank(args, "message_id")?),
            ]);
            insert_optional_non_blank_param(args, &mut params, "worker_id")?;
            insert_optional_bool_param(args, &mut params, "submit")?;
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
        "workflow_loop_set" => {
            reject_unexpected(
                args,
                &[
                    "workflow_id",
                    "recipe",
                    "stage",
                    "iteration",
                    "max_iterations",
                    "stop_reason",
                    "gates",
                ],
                name,
            )?;
            let mut params =
                map_from_pairs([("workflow_id", required_non_blank(args, "workflow_id")?)]);
            insert_optional_non_blank_param(args, &mut params, "recipe")?;
            insert_optional_non_blank_param(args, &mut params, "stage")?;
            insert_optional_u64_param(args, &mut params, "iteration")?;
            insert_optional_u64_param(args, &mut params, "max_iterations")?;
            insert_optional_non_blank_param(args, &mut params, "stop_reason")?;
            if let Some(gates) = args.get("gates").cloned() {
                if !gates.is_array() {
                    return Err(ToolCallError::validation("gates must be an array"));
                }
                params.insert("gates".to_string(), gates);
            }
            SocketCall {
                method: "workflow.loop.set",
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
            // `kind` and `title` are required by both the published input schema
            // and the socket server (workflow.evidence.add); enforce them here so
            // a non-schema-validating client gets a clear local invalid_params.
            let mut params = map_from_pairs([
                ("workflow_id", required_non_blank(args, "workflow_id")?),
                ("kind", required_non_blank(args, "kind")?),
                ("title", required_non_blank(args, "title")?),
            ]);
            for key in ["evidence_id", "path"] {
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
            // The socket server requires at least one of text/path; reject here
            // too so MCP validation fails early and consistently.
            if !params.contains_key("text") && !params.contains_key("path") {
                return Err(ToolCallError::validation(
                    "workflow_evidence_add requires text or path",
                ));
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

pub(super) fn tools_call_result_with_validation(
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

fn insert_optional_renamed_bool_param(
    args: &Map<String, Value>,
    params: &mut Map<String, Value>,
    source: &'static str,
    target: &'static str,
) -> Result<(), ToolCallError> {
    if let Some(value) = optional_bool(args, source)? {
        params.insert(target.to_string(), Value::Bool(value));
    }
    Ok(())
}

fn insert_optional_object_param(
    args: &Map<String, Value>,
    params: &mut Map<String, Value>,
    key: &'static str,
) -> Result<(), ToolCallError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Object(value)) => {
            params.insert(key.to_string(), Value::Object(value.clone()));
            Ok(())
        }
        Some(_) => Err(ToolCallError::validation(format!(
            "{key} must be an object"
        ))),
    }
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

fn has_workspace_selector_arg(args: &Map<String, Value>) -> bool {
    ["workspace_id", "workspace_name", "worktree_name"]
        .into_iter()
        .any(|key| args.contains_key(key))
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
        "context_snapshot" => "Built ForkTTY context snapshot.".to_string(),
        "task_strategy_plan" => "Planned ForkTTY task strategy.".to_string(),
        "task_strategy_apply" => "Processed ForkTTY task strategy request.".to_string(),
        "orchestration_cleanup" => "Processed ForkTTY orchestration cleanup request.".to_string(),
        "identify" => "Identified current ForkTTY workspace and surface context.".to_string(),
        "topology_tree" => "Built ForkTTY topology tree.".to_string(),
        "remote_list" => "Listed ForkTTY SSH remotes.".to_string(),
        "remote_status" => "Read ForkTTY SSH remote status.".to_string(),
        "agent_list" => "Listed ForkTTY agent sessions.".to_string(),
        "agent_health" => "Checked ForkTTY agent session readiness.".to_string(),
        "agent_reclaim_plan" => "Planned ForkTTY agent session reclaim candidates.".to_string(),
        "agent_hibernate" => "Hibernated ForkTTY agent session.".to_string(),
        "agent_reclaim" => "Reclaimed ForkTTY idle agent sessions.".to_string(),
        "agent_resume" => "Resumed ForkTTY agent session in a new tab.".to_string(),
        "team_list" => "Listed ForkTTY teams.".to_string(),
        "team_get" => "Read ForkTTY team state.".to_string(),
        "team_upsert" => "Updated ForkTTY team state.".to_string(),
        "team_finish" => "Finalized ForkTTY team state.".to_string(),
        "team_worker_upsert" => "Updated ForkTTY team worker state.".to_string(),
        "team_worker_heartbeat" => "Recorded ForkTTY team worker heartbeat.".to_string(),
        "team_worker_launch" => "Launched ForkTTY team worker pane.".to_string(),
        "team_worker_health" => "Read ForkTTY team worker health.".to_string(),
        "team_worker_nudge" => "Nudged ForkTTY team worker pane.".to_string(),
        "team_worker_shutdown" => "Requested ForkTTY team worker shutdown.".to_string(),
        "team_task_upsert" => "Updated ForkTTY team task state.".to_string(),
        "team_message_send" => "Queued ForkTTY team message.".to_string(),
        "team_message_dispatch" => {
            "Dispatched ForkTTY team message to a worker pane, optionally submitting it."
                .to_string()
        }
        "team_message_ack" => "Acknowledged ForkTTY team message.".to_string(),
        "team_inbox" => "Read ForkTTY team inbox.".to_string(),
        "team_summary" => "Summarized ForkTTY team state.".to_string(),
        "team_events" => "Listed ForkTTY team events.".to_string(),
        "status_summary" => "Built ForkTTY status summary.".to_string(),
        "workflow_list" => "Listed ForkTTY workflows.".to_string(),
        "workflow_get" => "Read ForkTTY workflow state.".to_string(),
        "workflow_upsert" => "Updated ForkTTY workflow state.".to_string(),
        "workflow_loop_set" => "Updated ForkTTY workflow loop state.".to_string(),
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
