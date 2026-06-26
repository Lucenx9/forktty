use super::*;

#[test]
fn browser_open_sends_browser_open_with_url_and_workspace() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"id":"s9","kind":{"type":"browser","url":"https://example.com"}},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                strings(&["open", "example.com", "--workspace-id", "w1"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.open");
    assert_eq!(request["params"]["url"], "example.com");
    assert_eq!(request["params"]["workspace_id"], "w1");
}

#[test]
fn browser_navigate_sends_browser_navigate_with_explicit_surface() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"navigated": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["navigate", "s9", "https://rust-lang.org"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.navigate");
    assert_eq!(request["params"]["surface_id"], "s9");
    assert_eq!(request["params"]["url"], "https://rust-lang.org");
}

#[test]
fn browser_rejects_unknown_subcommand() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(handle_browser(&ctx, strings(&["frobnicate"])), "browser");
}

#[test]
fn browser_requires_subcommand() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(handle_browser(&ctx, strings(&[])), "subcommand");
}

#[test]
fn socket_cli_compat_aliases_route_to_canonical_methods() {
    let requests = with_socket_server(
        2,
        |req| match req["method"].as_str() {
            Some("surface.list") => json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string(),
            Some("surface.send_text") => json!({
                "id": req["id"],
                "ok": true,
                "result": {"sent": true},
            })
            .to_string(),
            _ => json!({
                "id": req["id"],
                "ok": false,
                "error": {"code": "method_not_found", "message": "unexpected method"},
            })
            .to_string(),
        },
        |socket_path| {
            let mut surface_args =
                os_strings(&["surface:list", "--workspace-id", "w1", "--socket"]);
            surface_args.push(socket_path.as_os_str().to_os_string());
            run_inner(surface_args).unwrap();

            let mut send_text_args =
                os_strings(&["send_text", "hello", "--surface-id", "s1", "--socket"]);
            send_text_args.push(socket_path.as_os_str().to_os_string());
            run_inner(send_text_args).unwrap();
        },
    );

    assert_eq!(requests[0]["method"], "surface.list");
    assert_eq!(requests[0]["params"]["workspace_id"], "w1");
    assert_eq!(requests[1]["method"], "surface.send_text");
    assert_eq!(requests[1]["params"]["surface_id"], "s1");
    assert_eq!(requests[1]["params"]["text"], "hello");
}

#[test]
fn browser_rejects_blank_required_args_before_socket_use() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    for (args, expected) in [
        (
            strings(&["open", "   ", "--workspace-id", "w1"]),
            "browser open requires a URL",
        ),
        (
            strings(&["navigate", "   "]),
            "browser navigate requires a URL",
        ),
        (
            strings(&["click", "s9", "   "]),
            "browser click requires <surface-id> <ref>",
        ),
        (
            strings(&["profile", "create", "   "]),
            "browser profile create requires a <name>",
        ),
        (
            strings(&["history", "search", "   "]),
            "browser history search requires a <query>",
        ),
        (
            strings(&["bookmark", "add", "   "]),
            "browser bookmark add requires a <url>",
        ),
    ] {
        assert_err_contains(handle_browser(&ctx, args), expected);
    }
}

#[test]
fn browser_navigate_rejects_surface_id_flag() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(
            &ctx,
            strings(&["navigate", "--surface-id", "s9", "https://x.com"]),
        ),
        "browser navigate",
    );
}

#[test]
fn browser_click_rejects_extra_argument() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(&ctx, strings(&["click", "s9", "e3", "extra"])),
        "unexpected argument",
    );
}

#[test]
fn browser_fill_rejects_extra_argument() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(&ctx, strings(&["fill", "s9", "e3", "hello", "extra"])),
        "unexpected argument",
    );
}

#[test]
fn browser_open_resolves_active_workspace_when_id_omitted() {
    let requests = with_socket_server(
        2,
        |req| match req["method"].as_str() {
            Some("workspace.list") => json!({
                "id": req["id"],
                "ok": true,
                "result": [
                    { "id": "ws-idle", "active": false },
                    { "id": "ws-active", "active": true }
                ],
            })
            .to_string(),
            _ => json!({
                "id": req["id"],
                "ok": true,
                "result": {"id":"s1","kind":{"type":"browser","url":"https://example.com"}},
            })
            .to_string(),
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["open", "example.com"])).unwrap();
        },
    );
    assert_eq!(requests[0]["method"], "workspace.list");
    assert_eq!(requests[1]["method"], "browser.open");
    assert_eq!(requests[1]["params"]["workspace_id"], "ws-active");
    assert_eq!(requests[1]["params"]["url"], "example.com");
}

#[test]
fn browser_navigate_resolves_focused_surface_when_id_omitted() {
    let requests = with_socket_server(
        2,
        |req| match req["method"].as_str() {
            Some("workspace.list") => json!({
                "id": req["id"],
                "ok": true,
                "result": [
                    { "id": "a", "active": false, "focused_surface_id": "surface-a" },
                    { "id": "b", "active": true, "focused_surface_id": "surface-b" }
                ],
            })
            .to_string(),
            _ => json!({
                "id": req["id"],
                "ok": true,
                "result": {"navigated": true},
            })
            .to_string(),
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["navigate", "https://rust-lang.org"])).unwrap();
        },
    );
    assert_eq!(requests[0]["method"], "workspace.list");
    assert_eq!(requests[1]["method"], "browser.navigate");
    assert_eq!(requests[1]["params"]["surface_id"], "surface-b");
    assert_eq!(requests[1]["params"]["url"], "https://rust-lang.org");
}

#[test]
fn browser_open_errors_when_no_active_workspace() {
    let requests = with_socket_server(
        1,
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            assert_err_contains(
                handle_browser(&ctx, strings(&["open", "example.com"])),
                "no active workspace",
            );
        },
    );
    assert_eq!(requests[0]["method"], "workspace.list");
}

#[test]
fn browser_navigate_errors_when_no_focused_surface() {
    let requests = with_socket_server(
        1,
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            assert_err_contains(
                handle_browser(&ctx, strings(&["navigate", "https://x.com"])),
                "surface id",
            );
        },
    );
    assert_eq!(requests[0]["method"], "workspace.list");
}

#[test]
fn browser_snapshot_sends_request() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"role": "root", "children": []},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["snapshot", "s9"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.snapshot");
    assert_eq!(request["params"]["surface_id"], "s9");
}

#[test]
fn browser_click_sends_ref() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"ok": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["click", "s9", "e3"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.click");
    assert_eq!(request["params"]["surface_id"], "s9");
    assert_eq!(request["params"]["ref"], "e3");
}

#[test]
fn browser_fill_sends_ref_and_value() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"ok": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["fill", "s9", "e3", "hello world"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.fill");
    assert_eq!(request["params"]["surface_id"], "s9");
    assert_eq!(request["params"]["ref"], "e3");
    assert_eq!(request["params"]["value"], "hello world");
}

#[test]
fn browser_fill_reads_value_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let value_path = dir.path().join("value.txt");
    fs::write(&value_path, "secret from file").unwrap();
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"ok": true},
            })
            .to_string()
        },
        move |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                vec![
                    "fill".to_string(),
                    "s9".to_string(),
                    "e3".to_string(),
                    "--value-file".to_string(),
                    value_path.display().to_string(),
                ],
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.fill");
    assert_eq!(request["params"]["value"], "secret from file");
}

#[test]
fn browser_back_sends_request() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"ok": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["back", "s9"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.back");
    assert_eq!(request["params"]["surface_id"], "s9");
}

#[test]
fn browser_forward_sends_request() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"ok": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["forward", "s9"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.forward");
    assert_eq!(request["params"]["surface_id"], "s9");
}

#[test]
fn browser_reload_sends_request() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"ok": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["reload", "s9"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.reload");
    assert_eq!(request["params"]["surface_id"], "s9");
}

#[test]
fn browser_profile_list_sends_profile_list() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["profile", "list"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.profile.list");
    assert_eq!(request["params"], json!({}));
}

#[test]
fn browser_profile_create_sends_display_name() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"id": "p1", "display_name": "Work"},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["profile", "create", "Work"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.profile.create");
    assert_eq!(request["params"]["display_name"], "Work");
}

#[test]
fn browser_profile_create_missing_name_returns_error() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(&ctx, strings(&["profile", "create"])),
        "browser profile create requires a <name>",
    );
}

#[test]
fn browser_profile_delete_sends_id() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"deleted": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["profile", "delete", "p-abc123"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.profile.delete");
    assert_eq!(request["params"]["id"], "p-abc123");
}

#[test]
fn browser_open_with_profile_includes_profile_param() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"id": "s1", "kind": {"type": "browser", "url": "https://example.com"}},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                strings(&[
                    "open",
                    "--workspace-id",
                    "w1",
                    "--profile",
                    "Work",
                    "https://example.com",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.open");
    assert_eq!(request["params"]["url"], "https://example.com");
    assert_eq!(request["params"]["workspace_id"], "w1");
    assert_eq!(request["params"]["profile"], "Work");
}

#[test]
fn browser_history_list_sends_correct_method() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["history", "list"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.history.list");
    assert_eq!(request["params"], json!({}));
}

#[test]
fn browser_history_list_trims_profile_and_numeric_limit() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                strings(&["history", "list", "--profile", " Work ", "--limit", " 5 "]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.history.list");
    assert_eq!(request["params"]["profile"], "Work");
    assert_eq!(request["params"]["limit"], 5);
}

#[test]
fn browser_history_limit_requires_non_blank_number() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(&ctx, strings(&["history", "list", "--limit="])),
        "--limit requires a value",
    );
    assert_err_contains(
        handle_browser(&ctx, strings(&["history", "list", "--limit", "bad"])),
        "--limit must be a number",
    );
}

#[test]
fn browser_import_cli_is_not_exposed() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(&ctx, strings(&["import", "discover"])),
        "browser: unknown subcommand import",
    );
}

#[test]
fn browser_eval_cli_is_not_exposed() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(&ctx, strings(&["eval", "s9", "document.title"])),
        "browser: unknown subcommand eval",
    );
}

#[test]
fn browser_history_search_sends_query() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["history", "search", "foo"])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.history.search");
    assert_eq!(request["params"]["query"], "foo");
}

#[test]
fn browser_history_clear_sends_trimmed_profile() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"cleared": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["history", "clear", "--profile", " Work "])).unwrap();
        },
    );
    assert_eq!(request["method"], "browser.history.clear");
    assert_eq!(request["params"]["profile"], "Work");
}

#[test]
fn browser_history_search_missing_query_returns_error() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(&ctx, strings(&["history", "search"])),
        "browser history search requires a <query>",
    );
}

#[test]
fn browser_history_search_limit_is_numeric() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": [],
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                strings(&["history", "search", "hello", "--limit", "5"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.history.search");
    assert_eq!(request["params"]["query"], "hello");
    assert_eq!(request["params"]["limit"], json!(5));
}

#[test]
fn browser_history_search_invalid_limit_returns_error() {
    let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
    assert_err_contains(
        handle_browser(
            &ctx,
            strings(&["history", "search", "hello", "--limit", "bad"]),
        ),
        "--limit must be a number",
    );
}

#[test]
fn browser_bookmark_add_sends_url_and_title() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"added": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                strings(&[
                    "bookmark",
                    "add",
                    "https://example.com",
                    "--title",
                    "Example",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.bookmark.add");
    assert_eq!(request["params"]["url"], "https://example.com");
    assert_eq!(request["params"]["title"], "Example");
}

#[test]
fn browser_bookmark_add_trims_url_title_and_profile() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"added": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                strings(&[
                    "bookmark",
                    "add",
                    " https://example.com ",
                    "--title",
                    " Example ",
                    "--profile",
                    " Work ",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.bookmark.add");
    assert_eq!(request["params"]["url"], "https://example.com");
    assert_eq!(request["params"]["title"], "Example");
    assert_eq!(request["params"]["profile"], "Work");
}

#[test]
fn browser_bookmark_remove_sends_url() {
    let request = with_socket_response(
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": {"removed": true},
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(
                &ctx,
                strings(&["bookmark", "remove", "https://example.com"]),
            )
            .unwrap();
        },
    );
    assert_eq!(request["method"], "browser.bookmark.remove");
    assert_eq!(request["params"]["url"], "https://example.com");
}

#[test]
fn browser_bookmark_list_and_remove_trim_profile_and_url() {
    let requests = with_socket_server(
        2,
        |req| {
            json!({
                "id": req["id"],
                "ok": true,
                "result": if req["method"] == "browser.bookmark.list" {
                    json!([])
                } else {
                    json!({"removed": true})
                },
            })
            .to_string()
        },
        |socket_path| {
            let ctx = ctx_for(socket_path);
            handle_browser(&ctx, strings(&["bookmark", "list", "--profile", " Work "])).unwrap();
            handle_browser(
                &ctx,
                strings(&[
                    "bookmark",
                    "remove",
                    " https://example.com ",
                    "--profile",
                    " Work ",
                ]),
            )
            .unwrap();
        },
    );
    assert_eq!(requests[0]["method"], "browser.bookmark.list");
    assert_eq!(requests[0]["params"]["profile"], "Work");
    assert_eq!(requests[1]["method"], "browser.bookmark.remove");
    assert_eq!(requests[1]["params"]["url"], "https://example.com");
    assert_eq!(requests[1]["params"]["profile"], "Work");
}
