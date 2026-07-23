//! Runtime hook event handling, action construction, and token enrichment.

use crate::agent_guide;

use super::super::{
    now_nanos, read_optional_stdin_json, safe_string_field, sanitize_for_terminal,
    send_socket_request_with_timeout, socket_path_from_env, string_field, trimmed_env,
    write_stdout_line, write_stdout_text, CliContext, CliError, CliResult,
};
use super::install::{agent_spec, normalize_agent_name};
use super::{
    supported_agent_keys, AgentSpec, HOOK_CONTINUE_JSON, HOOK_EVENT_CLOCK, HOOK_EVENT_ORDER_PARAM,
    HOOK_STATUS_TIMEOUT,
};
use serde_json::{json, Map, Value};
use std::collections::VecDeque;
use std::path::Path;

mod payload;
mod token_usage;

pub(in crate::socket_cli) use payload::{
    extract_first_string_like, extract_hook_compact_trigger, extract_hook_message,
    extract_hook_permission_mode, extract_hook_session_id, extract_hook_source,
    extract_hook_tool_error, extract_hook_tool_name, extract_hook_turn_id, short_hash,
};
use payload::{extract_hook_notification_type, hook_notification_needs_attention};
pub(in crate::socket_cli) use token_usage::{
    format_token_usage_block, read_token_usage_from_transcript, resolve_token_ceiling, TokenUsage,
};

pub(in crate::socket_cli) fn handle_hook_event(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let agent_name = args
        .first()
        .map(|value| normalize_agent_name(value))
        .unwrap_or_default();
    let event = args
        .get(1)
        .map(|value| value.to_lowercase())
        .unwrap_or_default();
    // Real hooks always pass a known agent key from the generated templates,
    // so an unknown name here is a typed-in typo: fail loudly instead of
    // printing the lenient continue JSON and exiting 0.
    let Some(spec) = agent_spec(&agent_name) else {
        return Err(CliError::new(format!(
            "Unsupported hooks subcommand or agent: {}. Expected setup, remove, doctor, test, or `<agent> <event>` (agents: {})",
            args.first().map(String::as_str).unwrap_or("(missing)"),
            supported_agent_keys()
        )));
    };
    if !is_supported_hook_event(&event) {
        eprintln!(
            "{}",
            sanitize_for_terminal(&format!(
                "Unsupported hook event for {}: {}",
                spec.key,
                args.get(1).map(String::as_str).unwrap_or("(missing)")
            ))
        );
        print!("{HOOK_CONTINUE_JSON}");
        return Ok(());
    }
    if let Some(extra) = args.get(2) {
        eprintln!(
            "{}",
            sanitize_for_terminal(&format!(
                "Unexpected hook argument for {} {}: {}",
                spec.key, event, extra
            ))
        );
        print!("{HOOK_CONTINUE_JSON}");
        return Ok(());
    }

    if spec.key == "claude" && event == "session-start" && forktty_hook_context_from_env().is_none()
    {
        return write_stdout_text(HOOK_CONTINUE_JSON);
    }

    let payload = match read_optional_stdin_json() {
        Ok(payload) => payload,
        Err(err) => {
            eprintln!(
                "{}",
                sanitize_for_terminal(&format!(
                    "ForkTTY hook warning: failed to read hook stdin: {}",
                    err.message
                ))
            );
            Value::Null
        }
    };
    let order = next_hook_event_order();
    let has_socket_context = has_hook_socket_context(context);
    let has_explicit_hook_target = trimmed_env("FORKTTY_WORKSPACE_ID").is_some()
        || trimmed_env("FORKTTY_SURFACE_ID").is_some();
    let codex_session_provenance = local_codex_session_provenance(spec, &payload);
    let mut actions = build_hook_actions(spec, &event, &payload, &order);
    if !has_explicit_hook_target {
        add_codex_session_provenance(&mut actions, codex_session_provenance.as_ref());
    }
    let send_actions =
        should_send_hook_actions(context, spec, &payload, codex_session_provenance.as_ref());
    hook_debug(
        context,
        &format!(
            "{} {} order={} actions={} socket={}",
            spec.key,
            event,
            order,
            actions.len(),
            if send_actions {
                context.socket_path.display().to_string()
            } else {
                "(disabled)".to_string()
            }
        ),
    );
    if send_actions {
        for (method, params) in actions {
            if let Err(err) = send_socket_request_with_timeout(
                &context.socket_path,
                &method,
                params,
                HOOK_STATUS_TIMEOUT,
            ) {
                // Keep going on failure: a transient error on one action (e.g.
                // an informational log) must not skip later cleanup actions
                // like clearing a stale status or permission marker.
                let warning = format!("ForkTTY hook warning: {}", err.message);
                if has_socket_context {
                    eprintln!("{}", sanitize_for_terminal(&warning));
                } else {
                    hook_debug(context, &warning);
                }
            }
        }
    }
    let enrichments = gather_hook_enrichments(context, spec, &event, &payload);
    if let Some(token_action) =
        build_token_progress_action(spec, &enrichments, &event, &payload, &order)
    {
        if send_actions {
            let _ = send_socket_request_with_timeout(
                &context.socket_path,
                "metadata.set_progress",
                token_action,
                HOOK_STATUS_TIMEOUT,
            );
        }
    }
    let response = build_hook_response(spec, &event, &enrichments)?;
    write_stdout_line(&serde_json::to_string(&response)?)?;
    Ok(())
}

pub(in crate::socket_cli) fn is_supported_hook_event(event: &str) -> bool {
    matches!(
        event,
        "after-model"
            | "before-model"
            | "before-tool-selection"
            | "config-change"
            | "cwd-changed"
            | "elicitation"
            | "elicitation-result"
            | "file-changed"
            | "instructions-loaded"
            | "notification"
            | "permission-denied"
            | "permission-replied"
            | "permission-request"
            | "post-tool"
            | "post-tool-batch"
            | "post-tool-failure"
            | "pre-compact"
            | "pre-tool"
            | "post-compact"
            | "prompt-expansion"
            | "prompt-submit"
            | "session-end"
            | "session-start"
            | "setup"
            | "stop"
            | "stop-failure"
            | "subagent-start"
            | "subagent-stop"
            | "task-completed"
            | "task-created"
            | "teammate-idle"
            | "worktree-create"
            | "worktree-remove"
    )
}

pub(in crate::socket_cli) fn has_hook_socket_context(context: &CliContext) -> bool {
    context.socket_explicit || socket_path_from_env().is_some()
}

pub(in crate::socket_cli) fn should_send_hook_actions(
    context: &CliContext,
    spec: &AgentSpec,
    payload: &Value,
    codex_session_provenance: Option<&forktty_core::CodexTuiSessionProvenance>,
) -> bool {
    // Codex hooks can be launched by its long-lived shared app-server, which
    // does not inherit the environment of the TUI pane that owns the session.
    // Local Codex TUI session metadata plus cwd is enough to ask the
    // owner-only socket. The server still requires one matching surface and
    // one unclaimed live Codex TUI process before it accepts the fallback.
    has_hook_socket_context(context)
        || (spec.key == "codex"
            && extract_hook_session_id(payload).is_some()
            && codex_session_provenance.is_some())
}

fn local_codex_session_provenance(
    spec: &AgentSpec,
    payload: &Value,
) -> Option<forktty_core::CodexTuiSessionProvenance> {
    (spec.key == "codex")
        .then(|| extract_hook_session_id(payload))
        .flatten()
        .and_then(|session_id| forktty_core::codex_tui_session_provenance(&session_id))
}

fn add_codex_session_provenance(
    actions: &mut [(String, Value)],
    provenance: Option<&forktty_core::CodexTuiSessionProvenance>,
) {
    let Some(provenance) = provenance else {
        return;
    };
    for (_, params) in actions {
        if let Some(params) = params.as_object_mut() {
            params.insert(
                "hook_session_originator".to_string(),
                Value::String("codex-tui".to_string()),
            );
            params.insert(
                "hook_session_cwd".to_string(),
                Value::String(provenance.cwd().to_string_lossy().into_owned()),
            );
        }
    }
}

pub(in crate::socket_cli) fn hook_debug(context: &CliContext, message: &str) {
    if context.verbose || is_truthy_env("FORKTTY_HOOK_DEBUG") {
        eprintln!("ForkTTY hook debug: {}", sanitize_for_terminal(message));
    }
}

pub(in crate::socket_cli) fn is_truthy_env(key: &str) -> bool {
    trimmed_env(key)
        .map(|value| matches!(value.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Hook event ordering must survive wall-clock steps (NTP, manual `date`):
/// orders are compared across short-lived CLI processes, so use
/// CLOCK_BOOTTIME — system-wide, monotonic, and advancing across suspend —
/// instead of `SystemTime`, which previously dropped every hook update issued
/// after the clock stepped backwards.
pub(in crate::socket_cli) fn next_hook_event_order() -> String {
    boottime_nanos().to_string()
}

pub(in crate::socket_cli) fn boottime_nanos() -> u128 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec for the duration of the call.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) } == 0 {
        (ts.tv_sec as u128) * 1_000_000_000 + (ts.tv_nsec as u128)
    } else {
        // CLOCK_BOOTTIME cannot fail on the Linux kernels ForkTTY supports;
        // fall back to the wall clock rather than aborting a hook.
        now_nanos()
    }
}

pub(in crate::socket_cli) fn increment_hook_event_order(order: &str) -> String {
    order
        .parse::<u128>()
        .map(|value| value.saturating_add(1).to_string())
        .unwrap_or_else(|_| next_hook_event_order())
}

pub(in crate::socket_cli) fn hook_target_params() -> Map<String, Value> {
    let mut params = Map::new();
    if let Some(workspace_id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
        params.insert("workspace_id".to_string(), Value::String(workspace_id));
    }
    if let Some(surface_id) = trimmed_env("FORKTTY_SURFACE_ID") {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    }
    params
}

pub(in crate::socket_cli) fn add_hook_metadata(
    mut params: Map<String, Value>,
    spec: &AgentSpec,
    event: &str,
    payload: &Value,
    order: &str,
) -> Value {
    params.insert(
        "hook_agent".to_string(),
        Value::String(spec.key.to_string()),
    );
    params.insert(
        HOOK_EVENT_ORDER_PARAM.to_string(),
        Value::String(order.to_string()),
    );
    params.insert(
        "hook_event_clock".to_string(),
        Value::String(HOOK_EVENT_CLOCK.to_string()),
    );
    params.insert(
        "hook_event_name".to_string(),
        Value::String(event.to_string()),
    );
    if let Some(turn_id) = extract_hook_turn_id(event, payload) {
        params.insert("hook_turn_id".to_string(), Value::String(turn_id));
    }
    let session_id = extract_hook_session_id(payload);
    if let Some(session_id) = session_id.as_ref() {
        params.insert(
            "hook_session_id".to_string(),
            Value::String(session_id.clone()),
        );
    }
    if let (Some(kind), Some(session_id)) =
        (hook_prompt_kind(event, payload), session_id.as_deref())
    {
        let correlation_id = extract_hook_correlation_id(event, payload);
        let identity = correlation_id.as_deref().unwrap_or(order);
        let prompt_id = format!("{}/{}/{kind}/{identity}", spec.key, short_hash(session_id));
        params.insert(
            "hook_prompt_kind".to_string(),
            Value::String(kind.to_string()),
        );
        if let Some(correlation_id) = correlation_id {
            params.insert(
                "hook_correlation_id".to_string(),
                Value::String(correlation_id),
            );
        }
        params.insert("hook_prompt_id".to_string(), Value::String(prompt_id));
    }
    if let Some(cwd) = hook_session_cwd_for_metadata(spec, payload) {
        params.insert("hook_session_cwd".to_string(), Value::String(cwd));
    }
    Value::Object(params)
}

fn hook_prompt_kind(event: &str, payload: &Value) -> Option<&'static str> {
    match event {
        "permission-request" | "permission-replied" | "permission-result" | "permission-denied"
        | "post-tool-batch" => Some("permission"),
        "elicitation" | "elicitation-result" => Some("elicitation"),
        "notification" => match extract_hook_notification_type(payload).as_deref() {
            Some("permission_prompt") => Some("permission"),
            Some("elicitation_dialog") => Some("elicitation"),
            Some("idle_prompt") => Some("attention"),
            _ => None,
        },
        _ => None,
    }
}

fn extract_hook_correlation_id(event: &str, payload: &Value) -> Option<String> {
    let explicit = extract_first_string_like(
        payload,
        &[
            "prompt_id",
            "promptId",
            "request_id",
            "requestId",
            "requestID",
            "permission_id",
            "permissionId",
            "permissionID",
            "elicitation_id",
            "elicitationId",
            "tool_use_id",
            "toolUseId",
            "call_id",
            "callId",
        ],
    )
    .or_else(|| {
        (event == "permission-request")
            .then(|| {
                payload
                    .pointer("/raw/properties/id")
                    .or_else(|| payload.pointer("/properties/id"))
                    .and_then(value_as_non_blank_string)
            })
            .flatten()
    });
    explicit.map(|value| format!("id:{}", short_hash(&value)))
}

fn value_as_non_blank_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

pub(in crate::socket_cli) fn hook_session_cwd_for_metadata(
    spec: &AgentSpec,
    payload: &Value,
) -> Option<String> {
    if spec.key == "antigravity" {
        return extract_antigravity_workspace_cwd(payload);
    }
    std::env::current_dir()
        .ok()
        .filter(|cwd| !cwd.as_os_str().is_empty())
        .map(|cwd| cwd.to_string_lossy().into_owned())
}

pub(in crate::socket_cli) fn extract_antigravity_workspace_cwd(payload: &Value) -> Option<String> {
    extract_first_string_array_item(payload, &["workspacePaths", "workspace_paths"]).or_else(|| {
        extract_first_string_like(
            payload,
            &[
                "workspacePath",
                "workspace_path",
                "workspaceRoot",
                "workspace_root",
            ],
        )
        .and_then(|value| valid_hook_session_cwd(&value))
    })
}

pub(in crate::socket_cli) fn extract_first_string_array_item(
    payload: &Value,
    keys: &[&str],
) -> Option<String> {
    let mut queue = VecDeque::from([payload]);
    while let Some(current) = queue.pop_front() {
        let Some(object) = current.as_object() else {
            continue;
        };
        for key in keys {
            if let Some(Value::Array(values)) = object.get(*key) {
                for value in values {
                    if let Some(path) = value.as_str().and_then(valid_hook_session_cwd) {
                        return Some(path);
                    }
                }
            }
        }
        for value in object.values() {
            if value.is_object() {
                queue.push_back(value);
            }
        }
    }
    None
}

pub(in crate::socket_cli) fn valid_hook_session_cwd(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return None;
    }
    if !Path::new(trimmed).is_absolute() {
        return None;
    }
    Some(trimmed.to_string())
}

/// Map documented permission modes to a status color so risky modes are
/// visible at a glance. Unknown provider-specific values stay neutral
/// (`muted`) to avoid inventing a risk model the provider does not publish.
pub(in crate::socket_cli) fn permission_mode_color(spec: &AgentSpec, mode: &str) -> &'static str {
    if !matches!(spec.key, "claude" | "codex") {
        return "muted";
    }
    match mode {
        "bypassPermissions" | "dangerously-skip-permissions" => "red",
        "acceptEdits" | "auto" | "dontAsk" => "yellow",
        _ => "muted",
    }
}

fn permission_mode_is_bypass(mode: &str) -> bool {
    matches!(mode, "bypassPermissions" | "dangerously-skip-permissions")
}

pub(in crate::socket_cli) struct HookActionBuilder<'a> {
    spec: &'a AgentSpec,
    event: &'a str,
    payload: &'a Value,
    order: &'a str,
    target: Map<String, Value>,
    key: String,
    message: String,
    permission_key: String,
    permission_mode: Option<String>,
}

impl<'a> HookActionBuilder<'a> {
    fn new(spec: &'a AgentSpec, event: &'a str, payload: &'a Value, order: &'a str) -> Self {
        let target = hook_target_params();
        let key = format!("agent:{}", spec.key);
        let message = sanitize_for_terminal(&extract_hook_message(payload));
        let permission_key = format!("agent:{}:permission", spec.key);
        let permission_mode = extract_hook_permission_mode(payload);

        Self {
            spec,
            event,
            payload,
            order,
            target,
            key,
            message,
            permission_key,
            permission_mode,
        }
    }

    fn log(&self, level: &str, message: String) -> (String, Value) {
        let mut params = self.target.clone();
        params.insert("level".to_string(), Value::String(level.to_string()));
        params.insert("message".to_string(), Value::String(message));
        (
            "metadata.log".to_string(),
            add_hook_metadata(params, self.spec, self.event, self.payload, self.order),
        )
    }

    fn notification(&self, params: Map<String, Value>) -> (String, Value) {
        (
            "notification.create".to_string(),
            add_hook_metadata(params, self.spec, self.event, self.payload, self.order),
        )
    }

    fn status(&self, value: &str, color: &str, event_name: &str) -> (String, Value) {
        let mut params = self.target.clone();
        params.insert("key".to_string(), Value::String(self.key.clone()));
        params.insert(
            "label".to_string(),
            Value::String(self.spec.label.to_string()),
        );
        params.insert("value".to_string(), Value::String(value.to_string()));
        params.insert("color".to_string(), Value::String(color.to_string()));
        (
            "metadata.set_status".to_string(),
            add_hook_metadata(params, self.spec, event_name, self.payload, self.order),
        )
    }

    fn permission_status(&self, mode: &str, event_name: &str) -> (String, Value) {
        let mut params = self.target.clone();
        params.insert(
            "key".to_string(),
            Value::String(self.permission_key.clone()),
        );
        params.insert(
            "label".to_string(),
            Value::String(format!("{} mode", self.spec.label)),
        );
        params.insert("value".to_string(), Value::String(mode.to_string()));
        params.insert(
            "color".to_string(),
            Value::String(permission_mode_color(self.spec, mode).to_string()),
        );
        (
            "metadata.set_status".to_string(),
            add_hook_metadata(params, self.spec, event_name, self.payload, self.order),
        )
    }

    fn with_permission(
        &self,
        mut actions: Vec<(String, Value)>,
        event_name: &str,
    ) -> Vec<(String, Value)> {
        if let Some(mode) = self.permission_mode.as_deref() {
            actions.push(self.permission_status(mode, event_name));
        }
        actions
    }

    fn handle_session_start(&self) -> Vec<(String, Value)> {
        let source = extract_hook_source(self.payload)
            .map(|source| format!(" ({source})"))
            .unwrap_or_default();
        self.with_permission(
            vec![
                self.log(
                    "info",
                    format!("{} session started{source}", self.spec.label),
                ),
                self.status("Ready", "green", self.event),
            ],
            self.event,
        )
    }

    fn handle_prompt_submit(&self) -> Vec<(String, Value)> {
        self.with_permission(
            vec![
                self.log("info", format!("{} prompt submitted", self.spec.label)),
                self.status("Running", "blue", self.event),
            ],
            self.event,
        )
    }

    fn handle_notification(&self) -> Vec<(String, Value)> {
        if !hook_notification_needs_attention(self.payload, &self.message) {
            return vec![self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} notification", self.spec.label)
                } else {
                    self.message.clone()
                },
            )];
        }
        let mut note = self.target.clone();
        note.insert(
            "title".to_string(),
            Value::String(format!("{} needs input", self.spec.label)),
        );
        note.insert(
            "body".to_string(),
            Value::String(if self.message.is_empty() {
                format!("{} reported a prompt or attention event.", self.spec.label)
            } else {
                self.message.clone()
            }),
        );
        note.insert("kind".to_string(), Value::String("prompt".to_string()));
        vec![
            self.log(
                "warn",
                if self.message.is_empty() {
                    format!("{} requested attention", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Needs input", "yellow", self.event),
            self.notification(note),
        ]
    }

    fn handle_permission_request(&self) -> Vec<(String, Value)> {
        if let Some(mode) = self
            .permission_mode
            .as_deref()
            .filter(|mode| permission_mode_is_bypass(mode))
        {
            return vec![
                self.log(
                    "info",
                    format!(
                        "{} permission request observed while bypass mode is active",
                        self.spec.label
                    ),
                ),
                self.permission_status(mode, self.event),
            ];
        }
        let body = if self.message.is_empty() {
            format!("{} requested permission.", self.spec.label)
        } else {
            self.message.clone()
        };
        let mut note = self.target.clone();
        note.insert(
            "title".to_string(),
            Value::String(format!("{} permission required", self.spec.label)),
        );
        note.insert("body".to_string(), Value::String(body));
        note.insert("kind".to_string(), Value::String("prompt".to_string()));
        vec![
            self.log(
                "warn",
                if self.message.is_empty() {
                    format!("{} requested permission", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Permission required", "yellow", self.event),
            self.notification(note),
        ]
    }

    fn handle_permission_denied(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "warn",
                if self.message.is_empty() {
                    format!("{} permission denied", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Permission denied", "yellow", self.event),
        ]
    }

    fn handle_failure(&self) -> Vec<(String, Value)> {
        let body = if self.message.is_empty() {
            format!("{} reported a failure.", self.spec.label)
        } else {
            self.message.clone()
        };
        let mut note = self.target.clone();
        note.insert(
            "title".to_string(),
            Value::String(format!("{} error", self.spec.label)),
        );
        note.insert("body".to_string(), Value::String(body));
        note.insert("kind".to_string(), Value::String("error".to_string()));
        vec![
            self.log(
                "error",
                if self.message.is_empty() {
                    format!("{} reported a failure", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Error", "red", self.event),
            self.notification(note),
        ]
    }

    fn handle_basic_info(&self) -> Vec<(String, Value)> {
        vec![self.log(
            "info",
            if self.message.is_empty() {
                format!("{} reported {}", self.spec.label, self.event)
            } else {
                self.message.clone()
            },
        )]
    }

    fn handle_prompt_expansion(&self) -> Vec<(String, Value)> {
        vec![
            self.log("info", format!("{} prompt expansion", self.spec.label)),
            self.status("Running", "blue", self.event),
        ]
    }

    fn handle_before_model(&self) -> Vec<(String, Value)> {
        vec![
            self.log("info", format!("{} model request started", self.spec.label)),
            self.status("Thinking", "blue", self.event),
        ]
    }

    fn handle_after_model(&self) -> Vec<(String, Value)> {
        vec![self.log(
            "info",
            if self.message.is_empty() {
                format!("{} model response received", self.spec.label)
            } else {
                self.message.clone()
            },
        )]
    }

    fn handle_before_tool_selection(&self) -> Vec<(String, Value)> {
        vec![
            self.log("info", format!("{} selecting tools", self.spec.label)),
            self.status("Selecting tool", "blue", self.event),
        ]
    }

    fn handle_pre_tool(&self) -> Vec<(String, Value)> {
        let tool = extract_hook_tool_name(self.payload);
        vec![
            self.log(
                "info",
                tool.map(|tool| format!("{} running {tool}", self.spec.label))
                    .unwrap_or_else(|| format!("{} running tool", self.spec.label)),
            ),
            self.status("Running", "blue", self.event),
        ]
    }

    fn handle_post_tool(&self) -> Vec<(String, Value)> {
        let tool = extract_hook_tool_name(self.payload).unwrap_or_else(|| "tool".to_string());
        let is_error = extract_hook_tool_error(self.payload);
        let mut actions = vec![self.log(
            if is_error { "error" } else { "info" },
            if is_error {
                format!("{} {tool} reported an error", self.spec.label)
            } else {
                format!("{} finished {tool}", self.spec.label)
            },
        )];
        if is_error {
            let mut note = self.target.clone();
            note.insert(
                "title".to_string(),
                Value::String(format!("{} tool error", self.spec.label)),
            );
            note.insert(
                "body".to_string(),
                Value::String(format!("{tool} returned an error response.")),
            );
            note.insert("kind".to_string(), Value::String("error".to_string()));
            actions.push(self.notification(note));
        }
        actions
    }

    fn handle_post_tool_batch(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} finished tool batch", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Running", "blue", self.event),
        ]
    }

    fn handle_subagent_start(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} subagent started", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Subagent running", "blue", self.event),
        ]
    }

    fn handle_subagent_stop(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} subagent finished", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Running", "blue", self.event),
        ]
    }

    fn handle_task_created(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} task created", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Running", "blue", self.event),
        ]
    }

    fn handle_task_completed(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} task completed", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Running", "blue", self.event),
        ]
    }

    fn handle_elicitation(&self) -> Vec<(String, Value)> {
        let body = if self.message.is_empty() {
            format!("{} requested additional input.", self.spec.label)
        } else {
            self.message.clone()
        };
        let mut note = self.target.clone();
        note.insert(
            "title".to_string(),
            Value::String(format!("{} input required", self.spec.label)),
        );
        note.insert("body".to_string(), Value::String(body));
        note.insert("kind".to_string(), Value::String("prompt".to_string()));
        vec![
            self.log(
                "warn",
                if self.message.is_empty() {
                    format!("{} elicitation requested", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Needs input", "yellow", self.event),
            self.notification(note),
        ]
    }

    fn handle_elicitation_result(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} received {}", self.spec.label, self.event)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Running", "blue", self.event),
        ]
    }

    fn handle_pre_compact(&self) -> Vec<(String, Value)> {
        let trigger = extract_hook_compact_trigger(self.payload);
        let trigger_msg = trigger
            .as_ref()
            .map(|trigger| format!(" ({trigger})"))
            .unwrap_or_default();
        let mut note = self.target.clone();
        note.insert(
            "title".to_string(),
            Value::String(format!("{} compacting context", self.spec.label)),
        );
        note.insert(
            "body".to_string(),
            Value::String(
                trigger
                    .map(|trigger| format!("Context compaction triggered: {trigger}."))
                    .unwrap_or_else(|| "Context compaction in progress.".to_string()),
            ),
        );
        note.insert("kind".to_string(), Value::String("info".to_string()));
        vec![
            self.log(
                "warn",
                format!("{} context compacting{trigger_msg}", self.spec.label),
            ),
            self.status("Compacting", "yellow", self.event),
            self.notification(note),
        ]
    }

    fn handle_post_compact(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} context compacted", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Running", "blue", self.event),
        ]
    }

    fn clear_status(&self, key: &str) -> (String, Value) {
        let mut clear = self.target.clone();
        clear.insert("key".to_string(), Value::String(key.to_string()));
        (
            "metadata.clear_status".to_string(),
            add_hook_metadata(clear, self.spec, self.event, self.payload, self.order),
        )
    }

    fn handle_stop(&self) -> Vec<(String, Value)> {
        let mut actions = vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} stopped", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Ready", "green", self.event),
        ];
        if !self
            .spec
            .hook_entries
            .iter()
            .any(|entry| entry.hook_event_name == "session-end")
        {
            actions.push(self.clear_status(&self.permission_key));
        }
        actions
    }

    fn handle_teammate_idle(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} teammate idle", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Ready", "green", self.event),
        ]
    }

    fn handle_session_end(&self) -> Vec<(String, Value)> {
        vec![
            self.log("info", format!("{} session ended", self.spec.label)),
            self.clear_status(&self.key),
            self.clear_status(&self.permission_key),
        ]
    }

    fn build(self) -> Vec<(String, Value)> {
        match self.event {
            "session-start" => self.handle_session_start(),
            "prompt-submit" => self.handle_prompt_submit(),
            "notification" => self.handle_notification(),
            "permission-request" => self.handle_permission_request(),
            "permission-denied" => self.handle_permission_denied(),
            "stop-failure" | "post-tool-failure" => self.handle_failure(),
            "setup"
            | "config-change"
            | "instructions-loaded"
            | "cwd-changed"
            | "file-changed"
            | "worktree-create"
            | "worktree-remove" => self.handle_basic_info(),
            "prompt-expansion" => self.handle_prompt_expansion(),
            "before-model" => self.handle_before_model(),
            "after-model" => self.handle_after_model(),
            "before-tool-selection" => self.handle_before_tool_selection(),
            "pre-tool" => self.handle_pre_tool(),
            "post-tool" => self.handle_post_tool(),
            "post-tool-batch" => self.handle_post_tool_batch(),
            "subagent-start" => self.handle_subagent_start(),
            "subagent-stop" => self.handle_subagent_stop(),
            "task-created" => self.handle_task_created(),
            "task-completed" => self.handle_task_completed(),
            "elicitation" => self.handle_elicitation(),
            "elicitation-result" | "permission-replied" => self.handle_elicitation_result(),
            "pre-compact" => self.handle_pre_compact(),
            "post-compact" => self.handle_post_compact(),
            "stop" => self.handle_stop(),
            "teammate-idle" => self.handle_teammate_idle(),
            "session-end" => self.handle_session_end(),
            _ => Vec::new(),
        }
    }
}

pub(in crate::socket_cli) fn build_hook_actions(
    spec: &AgentSpec,
    event: &str,
    payload: &Value,
    order: &str,
) -> Vec<(String, Value)> {
    HookActionBuilder::new(spec, event, payload, order).build()
}

pub(in crate::socket_cli) struct HookEnrichments {
    pub(in crate::socket_cli) token_usage: Option<TokenUsage>,
    pub(in crate::socket_cli) workspace: Option<HookWorkspaceContext>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::socket_cli) struct ForkttyHookContext {
    pub(in crate::socket_cli) workspace_id: String,
    pub(in crate::socket_cli) surface_id: String,
    pub(in crate::socket_cli) socket_path: String,
}

pub(in crate::socket_cli) fn forktty_hook_context_from_env() -> Option<ForkttyHookContext> {
    Some(ForkttyHookContext {
        workspace_id: trimmed_env("FORKTTY_WORKSPACE_ID")?,
        surface_id: trimmed_env("FORKTTY_SURFACE_ID")?,
        socket_path: socket_path_from_env()?.display().to_string(),
    })
}

#[derive(Clone)]
pub(in crate::socket_cli) struct HookWorkspaceContext {
    pub(in crate::socket_cli) name: String,
    pub(in crate::socket_cli) git_branch: Option<String>,
}

pub(in crate::socket_cli) fn gather_hook_enrichments(
    context: &CliContext,
    spec: &AgentSpec,
    event: &str,
    payload: &Value,
) -> HookEnrichments {
    let mut enrichments = HookEnrichments {
        token_usage: None,
        workspace: None,
    };
    if spec.key != "claude" {
        return enrichments;
    }
    if event == "session-start" {
        let Some(hook_context) = forktty_hook_context_from_env() else {
            return enrichments;
        };
        enrichments.workspace = hook_workspace_context(context, &hook_context);
    }
    if event == "prompt-submit" {
        if let Some(path) =
            extract_first_string_like(payload, &["transcript_path", "transcriptPath"])
        {
            enrichments.token_usage = read_token_usage_from_transcript(Path::new(&path));
        }
    }
    enrichments
}

pub(in crate::socket_cli) fn hook_workspace_context(
    context: &CliContext,
    hook_context: &ForkttyHookContext,
) -> Option<HookWorkspaceContext> {
    if !has_hook_socket_context(context) {
        return None;
    }
    let workspaces = send_socket_request_with_timeout(
        &context.socket_path,
        "workspace.list",
        json!({}),
        HOOK_STATUS_TIMEOUT,
    )
    .ok()?;
    workspaces.as_array()?.iter().find_map(|workspace| {
        if string_field(workspace, "id") != Some(hook_context.workspace_id.as_str()) {
            return None;
        }
        let name = safe_string_field(workspace, "name")?;
        Some(HookWorkspaceContext {
            name,
            git_branch: safe_string_field(workspace, "gitBranch")
                .or_else(|| safe_string_field(workspace, "git_branch"))
                .filter(|branch| !branch.trim().is_empty()),
        })
    })
}

pub(in crate::socket_cli) fn build_token_progress_action(
    spec: &AgentSpec,
    enrichments: &HookEnrichments,
    event: &str,
    payload: &Value,
    order: &str,
) -> Option<Value> {
    let usage = enrichments.token_usage?;
    let total = usage.input_total();
    if total == 0 {
        return None;
    }
    let mut params = hook_target_params();
    params.insert(
        "key".to_string(),
        Value::String(format!("agent:{}:tokens", spec.key)),
    );
    params.insert(
        "label".to_string(),
        Value::String(format!("{} input tokens", spec.label)),
    );
    params.insert("value".to_string(), json!(total));
    params.insert("total".to_string(), json!(resolve_token_ceiling()));
    Some(add_hook_metadata(params, spec, event, payload, order))
}

pub(in crate::socket_cli) fn build_hook_response(
    spec: &AgentSpec,
    event: &str,
    enrichments: &HookEnrichments,
) -> CliResult<Value> {
    if spec.key == "claude" && event == "session-start" {
        let Some(hook_context) = forktty_hook_context_from_env() else {
            return serde_json::from_str(HOOK_CONTINUE_JSON.trim()).map_err(Into::into);
        };
        let workspace_line = enrichments
            .workspace
            .as_ref()
            .map(|workspace| {
                format!(
                    "Workspace: {}{}.",
                    workspace.name,
                    workspace
                        .git_branch
                        .as_deref()
                        .map(|branch| format!(" on branch {branch}"))
                        .unwrap_or_default()
                )
            })
            .unwrap_or_else(|| {
                format!("Workspace: {}; branch unknown.", hook_context.workspace_id)
            });
        let mut context_lines = vec![
            format!(
                "Running inside ForkTTY. workspace_id={} surface_id={} socket={}.",
                hook_context.workspace_id, hook_context.surface_id, hook_context.socket_path,
            ),
            workspace_line,
            "ForkTTY CLI/socket: context-snapshot gives a compact read-only view; list, surfaces, tree, read-screen, and capture-tail inspect panes; focus-surface and send-text target them.".to_string(),
            "Worktrees: forktty worktree-create opens an isolated git worktree workspace; worktree-attach, worktree-remove, and worktree-merge manage it.".to_string(),
            "Attention: forktty set-status, set-progress, log, and notify publish generic workspace metadata.".to_string(),
        ];
        context_lines.extend(
            agent_guide::session_context_lines()
                .into_iter()
                .map(str::to_string),
        );
        let additional_context = context_lines.join("\n");
        let mut hook_output = Map::new();
        hook_output.insert(
            "hookEventName".to_string(),
            Value::String("SessionStart".to_string()),
        );
        hook_output.insert(
            "additionalContext".to_string(),
            Value::String(additional_context),
        );
        if let Some(workspace) = &enrichments.workspace {
            hook_output.insert(
                "sessionTitle".to_string(),
                Value::String(workspace.name.clone()),
            );
        }
        return Ok(json!({
            "continue": true,
            "suppressOutput": false,
            "hookSpecificOutput": Value::Object(hook_output),
        }));
    }
    if spec.key == "claude" && event == "prompt-submit" {
        let mut sections = Vec::new();
        if let Some(usage) = enrichments.token_usage {
            sections.push(format_token_usage_block(usage));
        }
        if sections.is_empty() {
            return serde_json::from_str(HOOK_CONTINUE_JSON.trim()).map_err(Into::into);
        }
        return Ok(json!({
            "continue": true,
            "suppressOutput": false,
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": sections.join("\n\n"),
            }
        }));
    }
    // Antigravity unmarshals hook stdout with strict protojson and rejects
    // unknown fields ("continue" included), logging the hook as failed. Tool
    // hooks are gating hooks and need an explicit allow decision; non-gating
    // events can use an empty object as a no-op response.
    if spec.key == "antigravity" {
        if event == "pre-tool" {
            return Ok(json!({ "decision": "allow" }));
        }
        return Ok(json!({}));
    }
    serde_json::from_str(HOOK_CONTINUE_JSON.trim()).map_err(Into::into)
}
