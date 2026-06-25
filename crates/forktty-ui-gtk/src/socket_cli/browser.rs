use super::{
    insert_optional_trimmed_string_param, non_blank_string_option, parse_flags, parse_u64_option,
    print_json, print_result_or_json, read_text_file_or_stdin, reject_unknown_options,
    require_no_args, required_trimmed_arg, resolve_active_workspace_id, resolve_focused_surface_id,
    send_socket_request, string_field, string_option, write_stdout_line, CliContext, CliError,
    CliResult,
};
use serde_json::{json, Map, Value};

pub(super) fn handle_browser(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let mut iter = args.into_iter();
    let sub = iter.next().unwrap_or_default();
    let rest: Vec<String> = iter.collect();
    match sub.as_str() {
        "open" => browser_open(context, rest),
        "navigate" => browser_navigate(context, rest),
        "snapshot" => browser_surface_cmd(context, rest, "browser.snapshot", "snapshot", None),
        "click" => browser_click(context, rest),
        "fill" => browser_fill(context, rest),
        "back" => browser_surface_cmd(
            context,
            rest,
            "browser.back",
            "back",
            Some("Navigated back"),
        ),
        "forward" => browser_surface_cmd(
            context,
            rest,
            "browser.forward",
            "forward",
            Some("Navigated forward"),
        ),
        "reload" => browser_surface_cmd(
            context,
            rest,
            "browser.reload",
            "reload",
            Some("Reloaded"),
        ),
        "profile" => browser_profile(context, rest),
        "history" => browser_history(context, rest),
        "bookmark" => browser_bookmark(context, rest),
        "" => Err(CliError::new(
            "browser requires a subcommand: open | navigate | snapshot | click | fill | back | forward | reload | profile | history | bookmark",
        )),
        other => Err(CliError::new(format!(
            "browser: unknown subcommand {other}"
        ))),
    }
}

fn browser_open(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &["workspace-id", "axis", "profile"],
        "browser open",
    )?;
    let url = required_trimmed_arg(parsed.positionals.first(), "browser open requires a URL")?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "browser open: unexpected argument {}",
            parsed.positionals[1]
        )));
    }
    let workspace_id =
        match non_blank_string_option(&parsed.options, "workspace-id", "--workspace-id")? {
            Some(id) => id.trim().to_string(),
            None => resolve_active_workspace_id(context)?,
        };
    let mut params = Map::new();
    params.insert("url".to_string(), Value::String(url));
    params.insert("workspace_id".to_string(), Value::String(workspace_id));
    if let Some(axis) = non_blank_string_option(&parsed.options, "axis", "--axis")?.map(str::trim) {
        if !matches!(axis, "horizontal" | "vertical") {
            return Err(CliError::new(
                "Invalid --axis: expected horizontal or vertical",
            ));
        }
        params.insert("axis".to_string(), Value::String(axis.to_string()));
    }
    insert_optional_trimmed_string_param(
        &mut params,
        &parsed.options,
        "profile",
        "--profile",
        "profile",
    )?;
    let result = send_socket_request(&context.socket_path, "browser.open", Value::Object(params))?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format!(
            "Opened browser surface {}",
            string_field(&result, "id").unwrap_or("(unknown)")
        ))
    }
}

fn browser_navigate(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &[], "browser navigate")?;
    let (surface_id, url) = match parsed.positionals.as_slice() {
        [surface, url] => (
            required_trimmed_arg(Some(surface), "browser navigate requires a surface id")?,
            required_trimmed_arg(Some(url), "browser navigate requires a URL")?,
        ),
        [url] => {
            let url = required_trimmed_arg(Some(url), "browser navigate requires a URL")?;
            let surface = resolve_focused_surface_id(context)?
                .ok_or_else(|| CliError::new("browser navigate requires a surface id"))?;
            (surface, url)
        }
        [] => {
            return Err(CliError::new(
                "browser navigate requires [surface-id] <url>",
            ));
        }
        _ => return Err(CliError::new("browser navigate: too many arguments")),
    };
    let mut params = Map::new();
    params.insert("surface_id".to_string(), Value::String(surface_id));
    params.insert("url".to_string(), Value::String(url));
    let result = send_socket_request(
        &context.socket_path,
        "browser.navigate",
        Value::Object(params),
    )?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line("Navigated")
    }
}

fn browser_click(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &[], "browser click")?;
    let (surface_id, reference) = match parsed.positionals.as_slice() {
        [s, r] => (
            required_trimmed_arg(Some(s), "browser click requires <surface-id> <ref>")?,
            required_trimmed_arg(Some(r), "browser click requires <surface-id> <ref>")?,
        ),
        [_, _, extra, ..] => {
            return Err(CliError::new(format!(
                "browser click: unexpected argument '{extra}'"
            )));
        }
        _ => return Err(CliError::new("browser click requires <surface-id> <ref>")),
    };
    let result = send_socket_request(
        &context.socket_path,
        "browser.click",
        json!({"surface_id": surface_id, "ref": reference}),
    )?;
    print_result_or_json(context, "Clicked", result)
}

fn browser_fill(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &["value-file"], "browser fill")?;
    let value_file = string_option(&parsed.options, "value-file", "--value-file")?;
    let (surface_id, reference, value) = match parsed.positionals.as_slice() {
        [s, r] => {
            let Some(path) = value_file else {
                return Err(CliError::new(
                    "browser fill requires <surface-id> <ref> and <value> or --value-file",
                ));
            };
            (
                required_trimmed_arg(
                    Some(s),
                    "browser fill requires <surface-id> <ref> and <value> or --value-file",
                )?,
                required_trimmed_arg(
                    Some(r),
                    "browser fill requires <surface-id> <ref> and <value> or --value-file",
                )?,
                read_text_file_or_stdin(path, "browser fill value")?,
            )
        }
        [s, r, v] => {
            if value_file.is_some() {
                return Err(CliError::new(
                    "browser fill accepts either <value> or --value-file, not both",
                ));
            }
            (
                required_trimmed_arg(
                    Some(s),
                    "browser fill requires <surface-id> <ref> and <value> or --value-file",
                )?,
                required_trimmed_arg(
                    Some(r),
                    "browser fill requires <surface-id> <ref> and <value> or --value-file",
                )?,
                v.clone(),
            )
        }
        [_, _, _, extra, ..] => {
            return Err(CliError::new(format!(
                "browser fill: unexpected argument '{extra}'"
            )));
        }
        _ => {
            return Err(CliError::new(
                "browser fill requires <surface-id> <ref> and <value> or --value-file",
            ));
        }
    };
    let result = send_socket_request(
        &context.socket_path,
        "browser.fill",
        json!({"surface_id": surface_id, "ref": reference, "value": value}),
    )?;
    print_result_or_json(context, "Filled", result)
}

fn browser_surface_cmd(
    context: &CliContext,
    args: Vec<String>,
    method: &str,
    label: &str,
    human_message: Option<&str>,
) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(&parsed.options, &[], &format!("browser {label}"))?;
    let surface_id = required_trimmed_arg(
        parsed.positionals.first(),
        &format!("browser {label} requires a surface-id"),
    )?;
    if parsed.positionals.len() > 1 {
        return Err(CliError::new(format!(
            "browser {label}: unexpected argument '{}'",
            parsed.positionals[1]
        )));
    }
    let result = send_socket_request(
        &context.socket_path,
        method,
        json!({"surface_id": surface_id}),
    )?;
    match human_message {
        Some(message) => print_result_or_json(context, message, result),
        None => print_json(&result),
    }
}

fn browser_profile(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let mut iter = args.into_iter();
    let sub = iter.next().unwrap_or_default();
    let rest: Vec<String> = iter.collect();
    match sub.as_str() {
        "list" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(&parsed.options, &[], "browser profile list")?;
            require_no_args(&parsed.positionals, "browser profile list")?;
            let result =
                send_socket_request(&context.socket_path, "browser.profile.list", json!({}))?;
            print_json(&result)
        }
        "create" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(&parsed.options, &[], "browser profile create")?;
            let name = match parsed.positionals.as_slice() {
                [] => return Err(CliError::new("browser profile create requires a <name>")),
                [_, extra, ..] => {
                    return Err(CliError::new(format!(
                        "browser profile create: unexpected argument '{extra}'"
                    )));
                }
                [name] => {
                    required_trimmed_arg(Some(name), "browser profile create requires a <name>")?
                }
            };
            let result = send_socket_request(
                &context.socket_path,
                "browser.profile.create",
                json!({"display_name": name}),
            )?;
            print_json(&result)
        }
        "delete" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(&parsed.options, &[], "browser profile delete")?;
            let id = match parsed.positionals.as_slice() {
                [] => return Err(CliError::new("browser profile delete requires an <id>")),
                [_, extra, ..] => {
                    return Err(CliError::new(format!(
                        "browser profile delete: unexpected argument '{extra}'"
                    )));
                }
                [id] => required_trimmed_arg(Some(id), "browser profile delete requires an <id>")?,
            };
            let result = send_socket_request(
                &context.socket_path,
                "browser.profile.delete",
                json!({"id": id}),
            )?;
            print_json(&result)
        }
        "" => Err(CliError::new(
            "browser profile requires a subcommand: list | create | delete",
        )),
        other => Err(CliError::new(format!(
            "browser profile: unknown subcommand {other}"
        ))),
    }
}

fn browser_history(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let mut iter = args.into_iter();
    let sub = iter.next().unwrap_or_default();
    let rest: Vec<String> = iter.collect();
    match sub.as_str() {
        "list" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(
                &parsed.options,
                &["profile", "limit"],
                "browser history list",
            )?;
            require_no_args(&parsed.positionals, "browser history list")?;
            let mut params = Map::new();
            insert_optional_trimmed_string_param(
                &mut params,
                &parsed.options,
                "profile",
                "--profile",
                "profile",
            )?;
            if let Some(num) = parse_u64_option(&parsed.options, "limit", "--limit")? {
                params.insert("limit".to_string(), Value::Number(num.into()));
            }
            let result = send_socket_request(
                &context.socket_path,
                "browser.history.list",
                Value::Object(params),
            )?;
            print_json(&result)
        }
        "search" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(
                &parsed.options,
                &["profile", "limit"],
                "browser history search",
            )?;
            let query = required_trimmed_arg(
                parsed.positionals.first(),
                "browser history search requires a <query>",
            )?;
            if parsed.positionals.len() > 1 {
                return Err(CliError::new(format!(
                    "browser history search: unexpected argument '{}'",
                    parsed.positionals[1]
                )));
            }
            let mut params = Map::new();
            params.insert("query".to_string(), Value::String(query));
            insert_optional_trimmed_string_param(
                &mut params,
                &parsed.options,
                "profile",
                "--profile",
                "profile",
            )?;
            if let Some(num) = parse_u64_option(&parsed.options, "limit", "--limit")? {
                params.insert("limit".to_string(), Value::Number(num.into()));
            }
            let result = send_socket_request(
                &context.socket_path,
                "browser.history.search",
                Value::Object(params),
            )?;
            print_json(&result)
        }
        "clear" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(&parsed.options, &["profile"], "browser history clear")?;
            require_no_args(&parsed.positionals, "browser history clear")?;
            let mut params = Map::new();
            insert_optional_trimmed_string_param(
                &mut params,
                &parsed.options,
                "profile",
                "--profile",
                "profile",
            )?;
            let result = send_socket_request(
                &context.socket_path,
                "browser.history.clear",
                Value::Object(params),
            )?;
            print_json(&result)
        }
        "" => Err(CliError::new(
            "browser history requires a subcommand: list | search <query> | clear",
        )),
        other => Err(CliError::new(format!(
            "browser history: unknown subcommand {other}"
        ))),
    }
}

fn browser_bookmark(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let mut iter = args.into_iter();
    let sub = iter.next().unwrap_or_default();
    let rest: Vec<String> = iter.collect();
    match sub.as_str() {
        "add" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(
                &parsed.options,
                &["title", "profile"],
                "browser bookmark add",
            )?;
            let url = required_trimmed_arg(
                parsed.positionals.first(),
                "browser bookmark add requires a <url>",
            )?;
            if parsed.positionals.len() > 1 {
                return Err(CliError::new(format!(
                    "browser bookmark add: unexpected argument '{}'",
                    parsed.positionals[1]
                )));
            }
            let mut params = Map::new();
            params.insert("url".to_string(), Value::String(url));
            if let Some(t) = non_blank_string_option(&parsed.options, "title", "--title")? {
                params.insert("title".to_string(), Value::String(t.trim().to_string()));
            }
            insert_optional_trimmed_string_param(
                &mut params,
                &parsed.options,
                "profile",
                "--profile",
                "profile",
            )?;
            let result = send_socket_request(
                &context.socket_path,
                "browser.bookmark.add",
                Value::Object(params),
            )?;
            print_json(&result)
        }
        "list" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(&parsed.options, &["profile"], "browser bookmark list")?;
            require_no_args(&parsed.positionals, "browser bookmark list")?;
            let mut params = Map::new();
            insert_optional_trimmed_string_param(
                &mut params,
                &parsed.options,
                "profile",
                "--profile",
                "profile",
            )?;
            let result = send_socket_request(
                &context.socket_path,
                "browser.bookmark.list",
                Value::Object(params),
            )?;
            print_json(&result)
        }
        "remove" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(&parsed.options, &["profile"], "browser bookmark remove")?;
            let url = required_trimmed_arg(
                parsed.positionals.first(),
                "browser bookmark remove requires a <url>",
            )?;
            if parsed.positionals.len() > 1 {
                return Err(CliError::new(format!(
                    "browser bookmark remove: unexpected argument '{}'",
                    parsed.positionals[1]
                )));
            }
            let mut params = Map::new();
            params.insert("url".to_string(), Value::String(url));
            insert_optional_trimmed_string_param(
                &mut params,
                &parsed.options,
                "profile",
                "--profile",
                "profile",
            )?;
            let result = send_socket_request(
                &context.socket_path,
                "browser.bookmark.remove",
                Value::Object(params),
            )?;
            print_json(&result)
        }
        "" => Err(CliError::new(
            "browser bookmark requires a subcommand: add <url> | list | remove <url>",
        )),
        other => Err(CliError::new(format!(
            "browser bookmark: unknown subcommand {other}"
        ))),
    }
}
