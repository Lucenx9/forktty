//! Feed CLI commands and approval response formatting.

use super::{
    build_target_params, non_blank_string_option, parse_flags, parse_u64_option, print_json,
    reject_unknown_options, require_no_args, safe_string_field, send_socket_request,
    write_stdout_line, CliContext, CliError, CliResult, ParsedFlags,
};
use serde_json::{json, Value};

pub(super) fn handle_feed(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    if parsed
        .positionals
        .first()
        .is_some_and(|arg| arg == "respond")
    {
        return handle_feed_respond(context, parsed);
    }
    require_no_args(&parsed.positionals, "feed")?;
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "workspace-name", "worktree-name", "limit"],
        "feed",
    )?;
    let mut params = build_target_params(&parsed.options, "feed")?;
    if let Some(limit) = parse_u64_option(&parsed.options, "limit", "--limit")? {
        params.insert("limit".to_string(), Value::Number(limit.into()));
    }
    let result = send_socket_request(&context.socket_path, "feed.list", Value::Object(params))?;
    if context.json {
        return print_json(&result);
    }
    let Some(items) = result.as_array() else {
        return Ok(());
    };
    if items.is_empty() {
        return write_stdout_line("No feed items");
    }
    for item in items {
        write_stdout_line(&format_feed_line(item))?;
    }
    Ok(())
}

fn handle_feed_respond(context: &CliContext, parsed: ParsedFlags) -> CliResult<()> {
    if parsed.positionals.len() != 2 {
        return Err(CliError::new(
            "feed respond: expected feed respond <approval-id> --decision approve|deny",
        ));
    }
    reject_unknown_options(&parsed.options, &["decision"], "feed respond")?;
    let decision = non_blank_string_option(&parsed.options, "decision", "--decision")?
        .ok_or_else(|| CliError::new("feed respond: missing --decision approve|deny"))?;
    if !matches!(decision.trim(), "approve" | "approved" | "deny" | "denied") {
        return Err(CliError::new(
            "feed respond: --decision must be approve or deny",
        ));
    }
    let result = send_socket_request(
        &context.socket_path,
        "feed.approval.respond",
        json!({
            "id": parsed.positionals[1],
            "decision": decision,
        }),
    )?;
    if context.json {
        return print_json(&result);
    }
    write_stdout_line(&format!(
        "Recorded {} for {}",
        safe_string_field(&result, "approval_state").unwrap_or_else(|| "decision".to_string()),
        safe_string_field(&result, "id").unwrap_or_else(|| "approval".to_string())
    ))
}

fn format_feed_line(item: &Value) -> String {
    let item_type = safe_string_field(item, "type").unwrap_or_else(|| "item".to_string());
    let workspace = safe_string_field(item, "workspace_id").unwrap_or_else(|| "global".to_string());
    let title = safe_string_field(item, "title").unwrap_or_else(|| "(untitled)".to_string());
    let body = safe_string_field(item, "body")
        .filter(|body| !body.is_empty())
        .map(|body| format!(" — {body}"))
        .unwrap_or_default();
    format!("[{item_type}] {workspace} · {title}{body}")
}
