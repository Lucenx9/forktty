//! Low-level team task CLI commands.

use super::super::{
    comma_list_option, insert_optional_cli_raw_string_param, insert_optional_cli_string_param,
    parse_flags, print_result_or_json, reject_unknown_options, required_positionals,
    send_socket_request, CliContext, CliResult,
};
use super::format::format_team_task_line;
use serde_json::{Map, Value};

pub(in crate::socket_cli) fn handle_team_task_upsert(
    context: &CliContext,
    args: Vec<String>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "title",
            "status",
            "detail",
            "depends-on",
            "assigned-worker-id",
        ],
        "team-task-upsert",
    )?;
    let positionals = required_positionals(
        &parsed.positionals,
        "team-task-upsert",
        &["team-id", "task-id"],
    )?;
    let mut params = Map::new();
    params.insert("team_id".to_string(), Value::String(positionals[0].clone()));
    params.insert("task_id".to_string(), Value::String(positionals[1].clone()));
    insert_optional_cli_string_param(&mut params, &parsed.options, "title", "title")?;
    insert_optional_cli_string_param(&mut params, &parsed.options, "status", "status")?;
    insert_optional_cli_raw_string_param(&mut params, &parsed.options, "detail", "detail")?;
    if let Some(depends_on) = comma_list_option(&parsed.options, "depends-on", "--depends-on")? {
        params.insert(
            "depends_on".to_string(),
            Value::Array(depends_on.into_iter().map(Value::String).collect()),
        );
    }
    insert_optional_cli_string_param(
        &mut params,
        &parsed.options,
        "assigned-worker-id",
        "assigned_worker_id",
    )?;
    let result = send_socket_request(
        &context.socket_path,
        "team.task.upsert",
        Value::Object(params),
    )?;
    print_result_or_json(context, format_team_task_line(&result), result)
}
