//! Human-readable formatters for team CLI responses.

use super::super::{safe_string_field, sanitize_for_terminal};
use serde_json::Value;

pub(in crate::socket_cli) fn format_team_ask_flow_line(result: &Value) -> String {
    let launch = result.get("worker").unwrap_or(&Value::Null);
    let worker = launch.get("worker").unwrap_or(launch);
    let launch_surface = launch.get("surface").unwrap_or(&Value::Null);
    let task = result.get("task").unwrap_or(&Value::Null);
    let dispatch = result.get("dispatch").unwrap_or(&Value::Null);
    let worker_id = safe_string_field(worker, "id").unwrap_or_else(|| "(worker)".to_string());
    let agent = launch
        .get("selection")
        .and_then(|selection| safe_string_field(selection, "selected_agent"))
        .or_else(|| safe_string_field(worker, "agent"))
        .map(|agent| format!(" agent {agent}"))
        .unwrap_or_default();
    let task_id = safe_string_field(task, "id")
        .or_else(|| safe_string_field(worker, "assigned_task_id"))
        .map(|task| format!(" task {task}"))
        .unwrap_or_default();
    let surface_id = safe_string_field(dispatch, "surface_id")
        .or_else(|| safe_string_field(worker, "surface_id"))
        .or_else(|| safe_string_field(launch_surface, "id"))
        .map(|surface| format!(" surface {surface}"))
        .unwrap_or_default();
    let submit_state = if dispatch
        .get("submitted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "submitted"
    } else {
        "dispatched"
    };
    format!("Team prompt {submit_state} to {worker_id}{agent}{task_id}{surface_id}")
}

pub(in crate::socket_cli) fn format_team_line(team: &Value) -> String {
    let id = safe_string_field(team, "id").unwrap_or_else(|| "(unknown)".to_string());
    let name = safe_string_field(team, "name").unwrap_or_else(|| id.clone());
    let status = safe_string_field(team, "status").unwrap_or_else(|| "active".to_string());
    let workspace = safe_string_field(team, "workspace_id")
        .map(|workspace| format!(" [{workspace}]"))
        .unwrap_or_default();
    let leader = safe_string_field(team, "leader_surface_id")
        .map(|leader| format!(" leader {leader}"))
        .unwrap_or_default();
    let workers = team
        .get("workers")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let tasks = team
        .get("tasks")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let pending = team
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .filter(|message| {
                    !message
                        .get("delivered")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0);
    let goal = safe_string_field(team, "goal")
        .map(|goal| format!(" goal {goal}"))
        .unwrap_or_default();
    format!(
        "{id} {name}{workspace} {status}{leader} workers {workers} tasks {tasks} pending {pending}{goal}"
    )
}

pub(in crate::socket_cli) fn format_team_worker_line(worker: &Value) -> String {
    let id = safe_string_field(worker, "id").unwrap_or_else(|| "(worker)".to_string());
    let status = safe_string_field(worker, "status").unwrap_or_else(|| "idle".to_string());
    let role = safe_string_field(worker, "role")
        .map(|role| format!(" role {role}"))
        .unwrap_or_default();
    let agent = safe_string_field(worker, "agent")
        .map(|agent| format!(" agent {agent}"))
        .unwrap_or_default();
    let surface = safe_string_field(worker, "surface_id")
        .map(|surface| format!(" surface {surface}"))
        .unwrap_or_default();
    let worktree = safe_string_field(worker, "worktree_name")
        .map(|worktree| format!(" worktree {worktree}"))
        .unwrap_or_default();
    let task = safe_string_field(worker, "assigned_task_id")
        .map(|task| format!(" task {task}"))
        .unwrap_or_default();
    let heartbeat = worker
        .get("last_heartbeat_ms")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .map(|value| format!(" heartbeat {value}"))
        .unwrap_or_default();
    format!("worker {id} {status}{role}{agent}{surface}{worktree}{task}{heartbeat}")
}

pub(in crate::socket_cli) fn format_team_worker_launch_line(result: &Value) -> String {
    let worker = result.get("worker").unwrap_or(&Value::Null);
    let surface = result.get("surface").unwrap_or(&Value::Null);
    let worker_id = safe_string_field(worker, "id").unwrap_or_else(|| "(worker)".to_string());
    let surface_id = safe_string_field(surface, "id").unwrap_or_else(|| "(surface)".to_string());
    let agent = safe_string_field(worker, "agent")
        .or_else(|| {
            result
                .get("selection")
                .and_then(|selection| safe_string_field(selection, "selected_agent"))
        })
        .map(|agent| format!(" agent {agent}"))
        .unwrap_or_default();
    let role = safe_string_field(worker, "role")
        .map(|role| format!(" role {role}"))
        .unwrap_or_default();
    let task = safe_string_field(worker, "assigned_task_id")
        .map(|task| format!(" task {task}"))
        .unwrap_or_default();
    let argv = result
        .get("argv")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(sanitize_for_terminal)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let selection = result
        .get("selection")
        .and_then(|selection| {
            let requested = safe_string_field(selection, "requested_agent")?;
            let selected = safe_string_field(selection, "selected_agent")?;
            let reason = safe_string_field(selection, "reason").unwrap_or_default();
            (requested == "auto").then(|| {
                if reason.is_empty() {
                    format!(" selected {selected}")
                } else {
                    format!(" selected {selected} ({reason})")
                }
            })
        })
        .unwrap_or_default();
    format!("Launched worker {worker_id}{agent}{role}{task} in {surface_id}: {argv}{selection}")
}

pub(in crate::socket_cli) fn format_team_worker_health_line(worker: &Value) -> String {
    let id = safe_string_field(worker, "worker_id").unwrap_or_else(|| "(worker)".to_string());
    let lifecycle = safe_string_field(worker, "lifecycle").unwrap_or_else(|| "unknown".to_string());
    let final_state = safe_string_field(worker, "final_state").unwrap_or_else(|| lifecycle.clone());
    let status = safe_string_field(worker, "status").unwrap_or_else(|| "unknown".to_string());
    let surface = safe_string_field(worker, "surface_id")
        .map(|surface| format!(" surface {surface}"))
        .unwrap_or_default();
    let runtime = match (
        worker.get("surface_present").and_then(Value::as_bool),
        worker
            .get("surface_runtime_present")
            .and_then(Value::as_bool),
        worker.get("surface_ready").and_then(Value::as_bool),
    ) {
        (Some(true), _, Some(true)) => " runtime ready".to_string(),
        (Some(true), Some(true), Some(false)) => " runtime present/not-ready".to_string(),
        (Some(true), Some(false), _) | (Some(false), _, _) => " runtime missing".to_string(),
        _ => String::new(),
    };
    let heartbeat = worker
        .get("heartbeat_age_ms")
        .and_then(Value::as_u64)
        .map(|age| format!(" heartbeat_age_ms {age}"))
        .unwrap_or_default();
    format!(
        "worker {id} {lifecycle} final_state {final_state} status {status}{surface}{runtime}{heartbeat}"
    )
}

pub(in crate::socket_cli) fn format_team_task_line(task: &Value) -> String {
    let id = safe_string_field(task, "id").unwrap_or_else(|| "(task)".to_string());
    let title = safe_string_field(task, "title").unwrap_or_else(|| id.clone());
    let status = safe_string_field(task, "status").unwrap_or_else(|| "open".to_string());
    let worker = safe_string_field(task, "assigned_worker_id")
        .map(|worker| format!(" worker {worker}"))
        .unwrap_or_default();
    let depends = task
        .get("depends_on")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(sanitize_for_terminal)
                .collect::<Vec<_>>()
        })
        .filter(|items| !items.is_empty())
        .map(|items| format!(" depends {}", items.join(",")))
        .unwrap_or_default();
    format!("task {id} {status}{worker}{depends} {title}")
}

pub(in crate::socket_cli) fn format_team_message_line(message: &Value) -> String {
    let id = safe_string_field(message, "id").unwrap_or_else(|| "(message)".to_string());
    let from = safe_string_field(message, "from").unwrap_or_else(|| "(from)".to_string());
    let worker = safe_string_field(message, "to_worker_id")
        .map(|worker| format!(" to {worker}"))
        .unwrap_or_default();
    let task = safe_string_field(message, "task_id")
        .map(|task| format!(" task {task}"))
        .unwrap_or_default();
    let delivered = message
        .get("delivered")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let state = if delivered { "delivered" } else { "pending" };
    let body = safe_string_field(message, "body").unwrap_or_default();
    format!("message {id} {state} from {from}{worker}{task}: {body}")
}

pub(in crate::socket_cli) fn format_team_message_dispatch_line(result: &Value) -> String {
    let surface_id =
        safe_string_field(result, "surface_id").unwrap_or_else(|| "(surface)".to_string());
    let message = result.get("message").unwrap_or(&Value::Null);
    let message_id = safe_string_field(message, "id").unwrap_or_else(|| "(message)".to_string());
    if result
        .get("submitted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        format!("Dispatched message {message_id} to {surface_id} and submitted")
    } else {
        format!("Dispatched message {message_id} to {surface_id}")
    }
}

pub(in crate::socket_cli) fn format_team_summary_line(summary: &Value) -> String {
    let team_id = safe_string_field(summary, "team_id").unwrap_or_else(|| "(team)".to_string());
    let status = safe_string_field(summary, "status").unwrap_or_else(|| "active".to_string());
    let workers_total = summary
        .get("workers_total")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let workers_active = summary
        .get("workers_active")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let tasks_total = summary
        .get("tasks_total")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let tasks_open = summary
        .get("tasks_open")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let messages_pending = summary
        .get("messages_pending")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let last_event_seq = summary
        .get("last_event_seq")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    format!(
        "{team_id} {status} workers {workers_active}/{workers_total} tasks {tasks_open}/{tasks_total} pending {messages_pending} last_event {last_event_seq}"
    )
}

pub(in crate::socket_cli) fn format_team_finish_line(result: &Value) -> String {
    let team_id = safe_string_field(result, "team_id").unwrap_or_else(|| "(team)".to_string());
    let summary = result
        .get("summary_after")
        .or_else(|| result.get("summary_before"))
        .unwrap_or(&Value::Null);
    let status = safe_string_field(summary, "status").unwrap_or_else(|| {
        if result
            .get("finished")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "done".to_string()
        } else {
            "active".to_string()
        }
    });
    let action_count = result
        .get("actions")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    if result
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        format!("team {team_id} finish dry-run status {status} actions {action_count}")
    } else {
        format!("team {team_id} finished status {status} actions {action_count}")
    }
}

pub(in crate::socket_cli) fn format_team_event_line(event: &Value) -> String {
    let seq = event.get("seq").and_then(Value::as_u64).unwrap_or(0);
    let team_id = safe_string_field(event, "team_id").unwrap_or_else(|| "(team)".to_string());
    let kind = safe_string_field(event, "kind").unwrap_or_else(|| "team.event".to_string());
    let summary = safe_string_field(event, "summary").unwrap_or_default();
    format!("#{seq} {team_id} {kind} {summary}")
}
