//! Low-level team message and inbox CLI commands.

use super::super::{
    bool_option, insert_optional_cli_string_param, insert_optional_cli_u64_param,
    non_blank_string_option, parse_flags, print_json, print_result_or_json, reject_unknown_options,
    required_positionals, send_socket_request, string_option, trimmed_positional,
    write_stdout_line, CliContext, CliError, CliResult,
};
use super::format::{format_team_message_dispatch_line, format_team_message_line};
use serde_json::{Map, Value};

pub(in crate::socket_cli) fn handle_team_message_send(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &["message-id", "from", "to-worker-id", "task-id", "body"],
        "team-message-send",
    )?;
    if parsed.positionals.is_empty() {
        return Err(CliError::new("team-message-send requires team-id"));
    }
    let team_id = trimmed_positional(&parsed.positionals[0], "team-message-send", "team-id")?;
    let body = match string_option(&parsed.options, "body", "--body")? {
        Some(body) => {
            if parsed.positionals.len() > 1 {
                return Err(CliError::new(format!(
                    "team-message-send: unexpected argument {}",
                    parsed.positionals[1]
                )));
            }
            body.to_string()
        }
        None if parsed.positionals.len() > 1 => parsed.positionals[1..].join(" "),
        None => return Err(CliError::new("team-message-send requires --body")),
    };
    let from = non_blank_string_option(&parsed.options, "from", "--from")?
        .ok_or_else(|| CliError::new("team-message-send requires --from"))?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(team_id));
    params.insert("from".to_string(), Value::String(from.trim().to_string()));
    params.insert("body".to_string(), Value::String(body));
    insert_optional_cli_string_param(&mut params, &parsed.options, "message-id", "message_id")?;
    insert_optional_cli_string_param(&mut params, &parsed.options, "to-worker-id", "to_worker_id")?;
    insert_optional_cli_string_param(&mut params, &parsed.options, "task-id", "task_id")?;
    let result = send_socket_request(
        &context.socket_path,
        "team.message.send",
        Value::Object(params),
    )?;
    print_result_or_json(context, format_team_message_line(&result), result)
}

pub(in crate::socket_cli) fn handle_team_message_dispatch(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &["submit"]);
    reject_unknown_options(
        &parsed.options,
        &["worker-id", "submit"],
        "team-message-dispatch",
    )?;
    let positionals = required_positionals(
        &parsed.positionals,
        "team-message-dispatch",
        &["team-id", "message-id"],
    )?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    params.insert(
        "message_id".to_string(),
        Value::String(positionals[1].clone()),
    );
    insert_optional_cli_string_param(&mut params, &parsed.options, "worker-id", "worker_id")?;
    match bool_option(&parsed.options, "submit") {
        Some(true) => {
            params.insert("submit".to_string(), Value::Bool(true));
        }
        Some(false) => {}
        None => {
            return Err(CliError::new(
                "team-message-dispatch: --submit expects true or false",
            ));
        }
    }
    let result = send_socket_request(
        &context.socket_path,
        "team.message.dispatch",
        Value::Object(params),
    )?;
    print_result_or_json(context, format_team_message_dispatch_line(&result), result)
}

pub(in crate::socket_cli) fn handle_team_message_ack(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["worker-id"], "team-message-ack")?;
    let positionals = required_positionals(
        &parsed.positionals,
        "team-message-ack",
        &["team-id", "message-id"],
    )?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    params.insert(
        "message_id".to_string(),
        Value::String(positionals[1].clone()),
    );
    insert_optional_cli_string_param(&mut params, &parsed.options, "worker-id", "worker_id")?;
    let result = send_socket_request(
        &context.socket_path,
        "team.message.ack",
        Value::Object(params),
    )?;
    print_result_or_json(context, format_team_message_line(&result), result)
}

pub(in crate::socket_cli) fn handle_team_inbox(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &["include-delivered"]);
    reject_unknown_options(
        &parsed.options,
        &["worker-id", "include-delivered", "limit"],
        "team-inbox",
    )?;
    let positionals = required_positionals(&parsed.positionals, "team-inbox", &["team-id"])?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    insert_optional_cli_string_param(&mut params, &parsed.options, "worker-id", "worker_id")?;
    match bool_option(&parsed.options, "include-delivered") {
        Some(true) => {
            params.insert("include_delivered".to_string(), Value::Bool(true));
        }
        Some(false) => {}
        None => {
            return Err(CliError::new(
                "team-inbox: --include-delivered expects true or false",
            ));
        }
    }
    insert_optional_cli_u64_param(&mut params, &parsed.options, "limit", "limit")?;
    let result = send_socket_request(&context.socket_path, "team.inbox", Value::Object(params))?;
    if context.json {
        return print_json(&result);
    }
    let Some(messages) = result.as_array() else {
        return print_json(&result);
    };
    if messages.is_empty() {
        return write_stdout_line("No team messages");
    }
    for message in messages {
        write_stdout_line(&format_team_message_line(message))?;
    }
    Ok(())
}
