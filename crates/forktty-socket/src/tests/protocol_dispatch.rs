//! Dispatch parameter, request framing, and capability registry tests.

use super::*;

#[tokio::test]
async fn dispatch_returns_method_not_found_for_unknown_method() {
    let (state, _backend) = test_state();
    let err = dispatch(&state, "nonsense.bogus", json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "method_not_found");
    assert!(err.to_string().contains("nonsense.bogus"));
}

fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn random_param_value(rng: &mut u64, depth: u32) -> Value {
    // Keys real handlers look for, plus garbage; values span every JSON
    // type so each parameter extraction path sees the wrong type too.
    const KEYS: &[&str] = &[
        "surface_id",
        "surfaceId",
        "workspace",
        "workspace_id",
        "name",
        "cwd",
        "text",
        "host",
        "message",
        "title",
        "kind",
        "level",
        "label",
        "value",
        "id",
        "axis",
        "branch",
        "path",
        "url",
        "garbage \u{0} key",
    ];
    const STRINGS: &[&str] = &[
        "",
        " ",
        "x",
        "workspace-1",
        "surface-1",
        "../../../etc/passwd",
        "-1",
        "0",
        "999999999999999999999",
        "*",
        "/",
        "\\",
        "🦀\u{7f}",
        "{\"nested\":true}",
    ];
    let variants = if depth == 0 { 7 } else { 9 };
    match xorshift(rng) % variants {
        0 => Value::Null,
        1 => json!(true),
        2 => json!(-1),
        3 => json!(u64::MAX),
        4 => json!(f64::MAX),
        5 => json!(STRINGS[(xorshift(rng) % STRINGS.len() as u64) as usize]),
        6 => json!("a".repeat((xorshift(rng) % 8192) as usize)),
        7 => Value::Array(
            (0..xorshift(rng) % 3)
                .map(|_| random_param_value(rng, depth - 1))
                .collect(),
        ),
        _ => {
            let mut object = serde_json::Map::new();
            for _ in 0..xorshift(rng) % 4 {
                object.insert(
                    KEYS[(xorshift(rng) % KEYS.len() as u64) as usize].to_string(),
                    random_param_value(rng, depth - 1),
                );
            }
            Value::Object(object)
        }
    }
}

/// Deterministic randomized sweep over every dispatchable method with
/// adversarial params: the socket accepts NDJSON from any local client,
/// so no params shape may panic the server (errors are fine).
#[tokio::test]
#[serial_test::serial]
async fn dispatch_never_panics_on_adversarial_params() {
    let data_dir = tempfile::tempdir().unwrap();
    let _data_env = EnvGuard::set("XDG_DATA_HOME", data_dir.path().to_str().unwrap());

    let fixed = [
        Value::Null,
        json!({}),
        json!([]),
        json!(""),
        json!(0),
        json!(true),
        json!({"surface_id": null, "workspace": [], "text": {}}),
    ];
    let mut rng = 0x5eed_2026_0610_f00du64;
    for method in methods::capability_method_names() {
        // Fresh state per method so earlier mutations (closed workspaces,
        // split surfaces) cannot mask a later parameter path.
        let (state, _backend) = test_state();
        for params in &fixed {
            let _ = dispatch(&state, method, params.clone()).await;
        }
        for _ in 0..60 {
            let params = random_param_value(&mut rng, 2);
            let _ = dispatch(&state, method, params).await;
        }
    }
}

#[tokio::test]
async fn dispatch_returns_missing_param_for_workspace_command_without_selector() {
    let (state, _backend) = test_state();

    for method in ["workspace.select", "workspace.close"] {
        let err = dispatch(&state, method, json!({})).await.unwrap_err();
        assert_eq!(err.code(), "missing_param");
        assert!(err.to_string().contains("workspace selector"));
    }
}

#[tokio::test]
async fn dispatch_rejects_invalid_workspace_command_selectors() {
    let (state, _backend) = test_state();

    for workspace_id in [json!("  "), json!(42)] {
        let err = dispatch(
            &state,
            "workspace.select",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "invalid_param");
        assert!(err.to_string().contains("Invalid parameter workspace_id"));
    }

    let err = dispatch(
        &state,
        "workspace.select",
        json!({"workspace_id": "workspace-1", "workspace_name": "main"}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "invalid_param");
    assert!(err
        .to_string()
        .contains("Ambiguous workspace selector: cannot combine workspace_id and workspace_name"));
}

#[tokio::test]
async fn dispatch_returns_missing_param_for_send_text_without_text() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
    let err = dispatch(
        &state,
        "surface.send_text",
        json!({"surface_id": surface_id}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "missing_param");
    assert!(err.to_string().contains("text"));

    let err = dispatch(
        &state,
        "surface.send_text",
        json!({"surface_id": surface_id, "text": 42}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("Invalid parameter text"));

    let err = dispatch(
        &state,
        "surface.send_text",
        json!({"surface_id": surface_id, "text": ""}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("Invalid parameter text"));
}

#[tokio::test]
async fn dispatch_accepts_camel_case_surface_id_alias() {
    let (state, backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "surface.send_text",
        json!({"surfaceId": format!(" {surface_id}\n"), "text": "echo camel\n"}),
    )
    .await
    .unwrap();

    assert_eq!(backend.sent_text(surface_id).unwrap(), vec!["echo camel\n"]);
}

#[tokio::test]
async fn surface_commands_reject_invalid_surface_id_params() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    for (method, params, message) in [
        (
            "surface.send_text",
            json!({"surface_id": "", "text": "echo bad\n"}),
            "Invalid parameter surface_id",
        ),
        (
            "surface.send_text",
            json!({"surface_id": 42, "surfaceId": surface_id, "text": "echo bad\n"}),
            "Invalid parameter surface_id",
        ),
        (
            "surface.send_text",
            json!({"surfaceId": 42, "text": "echo bad\n"}),
            "Invalid parameter surfaceId",
        ),
        (
            "surface.split",
            json!({"surface_id": "", "axis": "vertical"}),
            "Invalid parameter surface_id",
        ),
        (
            "surface.focus",
            json!({"surface_id": 42}),
            "Invalid parameter surface_id",
        ),
        (
            "surface.close",
            json!({"surface_id": ""}),
            "Invalid parameter surface_id",
        ),
    ] {
        let err = dispatch(&state, method, params).await.unwrap_err();
        assert_eq!(err.code(), "invalid_param");
        assert!(err.to_string().contains(message));
    }

    let err = dispatch(
        &state,
        "surface.send_text",
        json!({
            "surface_id": surface_id,
            "surfaceId": surface_id,
            "text": "echo bad\n"
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "invalid_param");
    assert!(err
        .to_string()
        .contains("Ambiguous surface selector: cannot combine surface_id and surfaceId"));
}

#[tokio::test]
async fn dispatch_rejects_oversize_send_text_payload() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspaces[0]["focused_surface_id"]
        .as_str()
        .unwrap()
        .to_string();
    let huge = "x".repeat(MAX_SEND_TEXT_BYTES + 1);
    let err = dispatch(
        &state,
        "surface.send_text",
        json!({"surface_id": surface_id, "text": huge}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "payload_too_large");
}

#[test]
fn validates_worktree_name_params() {
    assert_eq!(validate_worktree_name(" feature/x ").unwrap(), "feature/x");
    let err = DispatchError::from(validate_worktree_name("../escape").unwrap_err());
    assert_eq!(err.code(), "invalid_param");
    assert!(validate_worktree_name("feature//empty").is_err());
    assert!(validate_worktree_name("feature\\windows").is_err());
    assert!(validate_worktree_name("-flag").is_err());
    assert!(validate_worktree_name("feature\nname").is_err());
    assert!(validate_worktree_name("").is_err());
}

#[test]
fn resolves_cwd_params_to_existing_directories() {
    let dir = tempfile::tempdir().unwrap();
    let resolved =
        path_resolver::resolve_workspace_cwd_param(&json!({"workingDir": dir.path()})).unwrap();
    assert_eq!(resolved, fs::canonicalize(dir.path()).unwrap());

    let missing = dir.path().join("missing");
    let error = path_resolver::resolve_cwd_param(&json!({"cwd": missing})).unwrap_err();
    assert!(error.contains("cannot resolve path"));
}

#[test]
fn rejects_ambiguous_directory_param_aliases() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();

    let workspace_error = path_resolver::resolve_workspace_cwd_param(&json!({
        "workingDir": first.path(),
        "cwd": second.path(),
    }))
    .unwrap_err();

    assert!(workspace_error.contains("Ambiguous path parameter"));
    assert!(workspace_error.contains("workingDir and cwd"));

    let repo_error = path_resolver::resolve_required_existing_dir_param(
        &json!({
            "path": first.path(),
            "cwd": second.path(),
        }),
        &["path", "cwd"],
        "path or cwd",
    )
    .unwrap_err();

    assert_eq!(repo_error.code(), "invalid_param");
    assert!(repo_error.to_string().contains("Ambiguous path parameter"));
    assert!(repo_error.to_string().contains("path and cwd"));
}

#[tokio::test]
async fn limited_line_rejects_oversize() {
    let data = b"abcdef\n";
    let mut reader = BufReader::new(std::io::Cursor::new(data.to_vec()));
    assert!(matches!(
        read_limited_line(&mut reader, 3).await,
        Some(Err(ReadLineError::TooLarge))
    ));
}

struct FailingAsyncBufRead;

impl tokio::io::AsyncRead for FailingAsyncBufRead {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Err(io::Error::other("read failed")))
    }
}

impl AsyncBufRead for FailingAsyncBufRead {
    fn poll_fill_buf(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<&[u8]>> {
        std::task::Poll::Ready(Err(io::Error::other("fill failed")))
    }

    fn consume(self: std::pin::Pin<&mut Self>, _amt: usize) {}
}

#[tokio::test]
async fn limited_line_surfaces_io_errors() {
    let mut reader = FailingAsyncBufRead;
    let result = read_limited_line(&mut reader, 3).await;

    match result {
        Some(Err(ReadLineError::Io(err))) => {
            assert_eq!(err.kind(), io::ErrorKind::Other);
            assert_eq!(err.to_string(), "fill failed");
        }
        other => panic!("expected read IO error, got {other:?}"),
    }
}

#[tokio::test]
async fn socket_connection_returns_structured_error_for_oversize_request() {
    let (state, _backend) = test_state();
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(handle_connection(server, state));
    let (read_half, mut write_half) = client.into_split();
    let oversize_request = vec![b'x'; MAX_REQUEST_SIZE + 1];

    write_half.write_all(&oversize_request).await.unwrap();
    write_half.write_all(b"\n").await.unwrap();
    write_half.shutdown().await.unwrap();

    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let response: JsonRpcResponse = serde_json::from_str(line.trim_end()).unwrap();

    assert!(!response.ok);
    assert_eq!(response.id, Value::Null);
    assert_eq!(response.error.unwrap().code, "payload_too_large");
    server.await.unwrap().unwrap();
}

#[cfg(feature = "browser")]
#[tokio::test]
async fn socket_connection_rejects_browser_import_methods() {
    let (state, _backend) = test_state();
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(handle_connection(server, state));
    let (read_half, mut write_half) = client.into_split();

    write_half
        .write_all(br#"{"id":1,"method":"browser.import.discover","params":{}}"#)
        .await
        .unwrap();
    write_half.write_all(b"\n").await.unwrap();
    write_half.shutdown().await.unwrap();

    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let response: JsonRpcResponse = serde_json::from_str(line.trim_end()).unwrap();

    assert!(!response.ok);
    assert_eq!(response.id, json!(1));
    assert_eq!(response.error.unwrap().code, "method_not_found");
    server.await.unwrap().unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn capabilities_lists_only_dispatchable_methods() {
    let (state, _backend) = test_state();
    let result = dispatch(&state, "system.capabilities", json!({}))
        .await
        .unwrap();
    let methods = result["methods"].as_array().unwrap();
    assert!(methods.iter().any(|m| m == "system.ping"));
    assert!(methods.iter().any(|m| m == "events.subscribe"));
    for removed_prefix in [
        "task.strategy.",
        "team.",
        "workflow.",
        "feed.",
        "orchestration.",
    ] {
        assert!(!methods.iter().any(|method| method
            .as_str()
            .is_some_and(|method| method.starts_with(removed_prefix))));
    }
    assert!(result.get("provider_capabilities").is_none());
    #[cfg(not(feature = "browser"))]
    assert!(!methods.iter().any(|m| {
        m.as_str()
            .is_some_and(|method| method.starts_with("browser."))
    }));
    // Every advertised method except the connection-level events.subscribe
    // must resolve to a dispatch arm (not MethodNotFound).
    for method in methods::capability_method_names() {
        if method == "events.subscribe" {
            continue;
        }
        if let Err(DispatchError::MethodNotFound(_)) = dispatch(&state, method, json!({})).await {
            panic!("advertised method {method} has no dispatch handler");
        }
    }
}

#[test]
fn method_registry_classifies_socket_exposure() {
    use methods::MethodExposure;

    let capability_methods = methods::capability_method_names();
    let mut seen = std::collections::BTreeSet::new();
    let mut all_specs = std::collections::BTreeSet::new();
    for spec in methods::method_specs() {
        assert!(
            all_specs.insert(spec.name),
            "duplicate method spec {}",
            spec.name
        );
    }
    for method in &capability_methods {
        assert!(seen.insert(*method), "duplicate capability method {method}");
        #[cfg(feature = "browser")]
        assert_ne!(
            methods::exposure(method),
            Some(MethodExposure::InternalOnly),
            "internal method advertised in capabilities: {method}"
        );
        assert!(
            method_allowed_from_socket(method),
            "capability method rejected by socket filter: {method}"
        );
    }
    assert_eq!(
        methods::exposure("events.subscribe"),
        Some(MethodExposure::ConnectionLevel)
    );
    assert!(method_allowed_from_socket("not.a.real.method"));

    #[cfg(feature = "browser")]
    {
        assert_eq!(
            methods::exposure("browser.open"),
            Some(MethodExposure::Public)
        );
        for method in [
            "browser.import.discover",
            "browser.import.preview",
            "browser.import.run",
        ] {
            assert_eq!(
                methods::exposure(method),
                Some(MethodExposure::InternalOnly)
            );
            assert!(!method_allowed_from_socket(method));
            assert!(
                !capability_methods.contains(&method),
                "internal method advertised in capabilities: {method}"
            );
        }
    }

    #[cfg(not(feature = "browser"))]
    {
        assert_eq!(methods::exposure("browser.open"), None);
        assert!(method_allowed_from_socket("browser.open"));
        assert!(!method_allowed_from_socket("browser.import.discover"));
    }
}

#[cfg(not(feature = "browser"))]
#[tokio::test]
async fn browser_methods_are_not_available_without_browser_feature() {
    let (state, _backend) = test_state();
    let err = dispatch(
        &state,
        "browser.open",
        json!({"workspace_id": "workspace-1", "url": "https://example.com"}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "method_not_found");
}
