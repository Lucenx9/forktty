//! Runtime hook event handling, action construction, and token enrichment.

use crate::agent_guide;

use super::super::{
    now_nanos, read_optional_stdin_json, safe_string_field, sanitize_for_terminal,
    send_socket_request_with_timeout, socket_path_from_env, string_field, trimmed_env,
    write_stdout_line, CliContext, CliError, CliResult,
};
use super::install::{agent_spec, normalize_agent_name};
use super::{
    supported_agent_keys, AgentSpec, HOOK_CONTINUE_JSON, HOOK_EVENT_CLOCK, HOOK_EVENT_ORDER_PARAM,
    HOOK_STATUS_TIMEOUT, HOOK_TOKEN_CEILING_DEFAULT, HOOK_TOOL_LABEL_MAX,
};
use serde_json::{json, Map, Value};
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

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
    let actions = build_hook_actions(spec, &event, &payload, &order);
    hook_debug(
        context,
        &format!(
            "{} {} order={} actions={} socket={}",
            spec.key,
            event,
            order,
            actions.len(),
            if should_send_hook_actions(context) {
                context.socket_path.display().to_string()
            } else {
                "(disabled)".to_string()
            }
        ),
    );
    if should_send_hook_actions(context) {
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
                eprintln!(
                    "{}",
                    sanitize_for_terminal(&format!("ForkTTY hook warning: {}", err.message))
                );
            }
        }
    }
    let enrichments = gather_hook_enrichments(context, spec, &event, &payload);
    if let Some(token_action) = build_token_progress_action(spec, &enrichments, &event, &order) {
        if should_send_hook_actions(context) {
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

pub(in crate::socket_cli) fn should_send_hook_actions(context: &CliContext) -> bool {
    context.socket_explicit || socket_path_from_env().is_some()
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
    if let Some(session_id) = extract_hook_session_id(payload) {
        params.insert("hook_session_id".to_string(), Value::String(session_id));
    }
    if let Some(cwd) = hook_session_cwd_for_metadata(spec, payload) {
        params.insert("hook_session_cwd".to_string(), Value::String(cwd));
    }
    Value::Object(params)
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
        "bypassPermissions" => "red",
        "acceptEdits" | "auto" | "dontAsk" => "yellow",
        _ => "muted",
    }
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
        ("metadata.log".to_string(), Value::Object(params))
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
            ("notification.create".to_string(), Value::Object(note)),
        ]
    }

    fn handle_permission_request(&self) -> Vec<(String, Value)> {
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
            ("notification.create".to_string(), Value::Object(note)),
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
            ("notification.create".to_string(), Value::Object(note)),
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
            actions.push(("notification.create".to_string(), Value::Object(note)));
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
            ("notification.create".to_string(), Value::Object(note)),
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
            self.status("Running", "blue", self.event),
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

#[derive(Clone)]
pub(in crate::socket_cli) struct HookWorkspaceContext {
    pub(in crate::socket_cli) name: String,
    pub(in crate::socket_cli) git_branch: Option<String>,
}

#[derive(Clone, Copy)]
pub(in crate::socket_cli) struct TokenUsage {
    pub(in crate::socket_cli) input: u64,
    pub(in crate::socket_cli) output: u64,
    pub(in crate::socket_cli) cache_read: u64,
    pub(in crate::socket_cli) cache_creation: u64,
}

impl TokenUsage {
    fn input_total(self) -> u64 {
        self.input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_creation)
    }
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
        enrichments.workspace = hook_workspace_context(context);
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
) -> Option<HookWorkspaceContext> {
    let workspace_id = trimmed_env("FORKTTY_WORKSPACE_ID")?;
    if !should_send_hook_actions(context) {
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
        if string_field(workspace, "id") != Some(workspace_id.as_str()) {
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
    Some(add_hook_metadata(params, spec, event, &Value::Null, order))
}

pub(in crate::socket_cli) fn build_hook_response(
    spec: &AgentSpec,
    event: &str,
    enrichments: &HookEnrichments,
) -> CliResult<Value> {
    if spec.key == "claude" && event == "session-start" {
        let workspace_id =
            trimmed_env("FORKTTY_WORKSPACE_ID").unwrap_or_else(|| "(none)".to_string());
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
            .unwrap_or_else(|| format!("Workspace: {workspace_id}; branch unknown."));
        let mut context_lines = vec![
            format!(
                "Running inside ForkTTY. workspace_id={} surface_id={} socket={}.",
                workspace_id,
                trimmed_env("FORKTTY_SURFACE_ID").unwrap_or_else(|| "(none)".to_string()),
                trimmed_env("FORKTTY_SOCKET_PATH").unwrap_or_else(|| "(default)".to_string()),
            ),
            workspace_line,
            "MCP tools: context_snapshot gives a compact read-only view; workspace_list, surface_list, topology_tree, surface_read_text, and surface_capture_tail inspect panes; surface_focus and surface_send_text drive them.".to_string(),
            "Worktrees: worktree_create creates an isolated git worktree + workspace; worktree_attach, worktree_remove, and worktree_merge manage branches.".to_string(),
            "Status: status_set and notification_create publish progress; CLI fallback is forktty list/surfaces/send-text/worktree-*.".to_string(),
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
        if trimmed_env("FORKTTY_WORKSPACE_ID").is_some() {
            if let Some(workspace) = &enrichments.workspace {
                hook_output.insert(
                    "sessionTitle".to_string(),
                    Value::String(workspace.name.clone()),
                );
            }
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

pub(in crate::socket_cli) fn extract_hook_message(payload: &Value) -> String {
    extract_first_string(
        payload,
        &[
            "message",
            "body",
            "reason",
            "error",
            "summary",
            "detail",
            "title",
            "text",
            "last_assistant_message",
        ],
    )
    .unwrap_or_default()
}

pub(in crate::socket_cli) fn extract_hook_source(payload: &Value) -> Option<String> {
    extract_first_string_like(payload, &["source", "trigger", "reason"])
        .map(|value| sanitize_for_terminal(&value).chars().take(32).collect())
}

pub(in crate::socket_cli) fn extract_hook_compact_trigger(payload: &Value) -> Option<String> {
    extract_first_string_like(
        payload,
        &["trigger", "compact_trigger", "compactTrigger", "reason"],
    )
    .map(|value| sanitize_for_terminal(&value).chars().take(32).collect())
}

pub(in crate::socket_cli) fn extract_hook_tool_name(payload: &Value) -> Option<String> {
    let sanitized = sanitize_for_terminal(&extract_first_string_like(
        payload,
        &["tool_name", "toolName", "tool", "name"],
    )?);
    if sanitized.chars().count() <= HOOK_TOOL_LABEL_MAX {
        Some(sanitized)
    } else {
        Some(format!(
            "{}...",
            sanitized
                .chars()
                .take(HOOK_TOOL_LABEL_MAX.saturating_sub(3))
                .collect::<String>()
        ))
    }
}

pub(in crate::socket_cli) fn extract_hook_tool_error(payload: &Value) -> bool {
    // Inspect only the documented error container — the tool result object
    // (`tool_response`) and, as a fallback, the payload root — one level deep.
    //
    // A previous version walked the *entire* payload recursively and flagged an
    // error on any `error`/`is_error`/`isError` key anywhere inside it. Codex
    // PostToolUse payloads carry rich, nested tool output (e.g. MCP
    // `structuredContent`, JSON-emitting commands) that legitimately contains
    // nested `error` keys even on success, so the recursive scan produced
    // spurious "error" log lines and notifications on routine Codex use. Both
    // the Claude (`tool_response.is_error`) and MCP (`tool_response.isError`)
    // contracts expose the flag at the top of the response, so a single-level
    // check is sufficient and far less noisy.
    [payload.get("tool_response"), Some(payload)]
        .into_iter()
        .flatten()
        .any(object_signals_tool_error)
}

pub(in crate::socket_cli) fn object_signals_tool_error(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    for key in ["is_error", "isError", "error"] {
        match object.get(key) {
            Some(Value::Bool(true)) => return true,
            Some(Value::String(value)) if !value.trim().is_empty() => return true,
            Some(Value::Object(value))
                if value.contains_key("message")
                    || value.contains_key("type")
                    || value.contains_key("code") =>
            {
                return true
            }
            _ => {}
        }
    }
    false
}

pub(in crate::socket_cli) fn extract_hook_permission_mode(payload: &Value) -> Option<String> {
    extract_first_string_like(payload, &["permission_mode", "permissionMode"])
        .map(|value| {
            sanitize_for_terminal(&value)
                .chars()
                .take(64)
                .collect::<String>()
        })
        .filter(|value| !value.is_empty())
}

pub(in crate::socket_cli) fn extract_hook_session_id(payload: &Value) -> Option<String> {
    // conversationId is Antigravity's session identifier.
    extract_first_string_like(
        payload,
        &[
            "session_id",
            "sessionId",
            "sessionID",
            "conversationId",
            "conversation_id",
        ],
    )
    .map(|value| {
        sanitize_for_terminal(&value)
            .chars()
            .take(96)
            .collect::<String>()
    })
    .filter(|value| !value.is_empty())
}

pub(in crate::socket_cli) fn extract_hook_turn_id(event: &str, payload: &Value) -> Option<String> {
    let explicit = extract_first_string_like(
        payload,
        &[
            "turn_id",
            "turnId",
            "prompt_id",
            "promptId",
            "request_id",
            "requestId",
            "message_id",
            "messageId",
            "event_id",
            "eventId",
            "sequence",
            "seq",
        ],
    );
    if let Some(explicit) = explicit {
        return Some(format!("id:{}", short_hash(&explicit)));
    }
    if event != "prompt-submit" {
        return None;
    }
    extract_first_string_like(payload, &["prompt", "message", "text", "body"])
        .map(|prompt| format!("prompt:{}", short_hash(&prompt)))
}

pub(in crate::socket_cli) fn extract_first_string(
    payload: &Value,
    keys: &[&str],
) -> Option<String> {
    let mut queue = VecDeque::from([payload]);
    while let Some(current) = queue.pop_front() {
        let Some(object) = current.as_object() else {
            continue;
        };
        for key in keys {
            if let Some(value) = object.get(*key).and_then(Value::as_str) {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
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

pub(in crate::socket_cli) fn extract_first_string_like(
    payload: &Value,
    keys: &[&str],
) -> Option<String> {
    let mut queue = VecDeque::from([payload]);
    while let Some(current) = queue.pop_front() {
        let Some(object) = current.as_object() else {
            continue;
        };
        for key in keys {
            match object.get(*key) {
                Some(Value::String(value)) if !value.trim().is_empty() => {
                    return Some(value.trim().to_string())
                }
                Some(Value::Number(value)) => return Some(value.to_string()),
                Some(Value::Bool(value)) => return Some(value.to_string()),
                _ => {}
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

pub(in crate::socket_cli) fn short_hash(value: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub(in crate::socket_cli) fn read_token_usage_from_transcript(path: &Path) -> Option<TokenUsage> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let size = metadata.len();
    if size == 0 {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let chunk_size = size.min(64 * 1024);
    file.seek(SeekFrom::Start(size - chunk_size)).ok()?;
    let mut buffer = vec![0; chunk_size as usize];
    file.read_exact(&mut buffer).ok()?;
    let text = String::from_utf8_lossy(&buffer);
    for raw in text.lines().rev() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(usage) = entry
            .get("message")
            .and_then(|message| message.get("usage"))
            .or_else(|| entry.get("usage"))
        else {
            continue;
        };
        return Some(TokenUsage {
            input: usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output: usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_read: usage
                .get("cache_read_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_creation: usage
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        });
    }
    None
}

pub(in crate::socket_cli) fn resolve_token_ceiling() -> u64 {
    trimmed_env("FORKTTY_HOOK_TOKEN_CEILING")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(HOOK_TOKEN_CEILING_DEFAULT)
}

pub(in crate::socket_cli) fn format_token_usage_block(usage: TokenUsage) -> String {
    let total = usage.input_total();
    let ceiling = resolve_token_ceiling();
    let pct = if ceiling > 0 {
        ((total as f64 / ceiling as f64) * 100.0).round().min(100.0) as u64
    } else {
        0
    };
    format!(
        "ForkTTY token estimate (latest assistant turn): ~{} / {} input tokens ({}% — input={}, cache_read={}, cache_creation={}, output={}).",
        format_thousands(total),
        format_thousands(ceiling),
        pct,
        usage.input,
        usage.cache_read,
        usage.cache_creation,
        usage.output,
    )
}

pub(in crate::socket_cli) fn format_thousands(value: u64) -> String {
    let text = value.to_string();
    let mut out = String::new();
    for (index, ch) in text.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}
