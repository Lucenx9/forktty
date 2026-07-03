//! Socket provider capability and team worker launch command tests.

use super::*;

#[test]
fn team_worker_launch_rejects_removed_gemini_provider() {
    let err = team_worker_launch_command("gemini", None, Vec::new()).unwrap_err();
    assert!(
        err.to_string()
            .contains("unsupported team worker agent: gemini"),
        "{err}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn capabilities_report_provider_detection_and_team_policy() {
    let dir = tempfile::tempdir().unwrap();
    let codex = write_fake_program(dir.path(), "codex");
    let pi = write_fake_program(dir.path(), "pi");
    let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
    let config_home = tempfile::tempdir().unwrap();
    let _config_home = EnvGuard::set("XDG_CONFIG_HOME", config_home.path().to_str().unwrap());
    let (state, _backend) = test_state();

    let result = dispatch(&state, "system.capabilities", json!({}))
        .await
        .unwrap();

    let policy = &result["team_provider_policy"];
    assert_eq!(policy["default_agent"], "auto");
    assert_eq!(
        policy["provider_order"],
        json!(["codex", "claude", "pi", "opencode", "antigravity", "grok"])
    );
    assert_eq!(policy["auto_fallback"], true);
    assert_eq!(policy["disabled_agents"], json!([]));

    let providers = &result["provider_capabilities"];
    assert_eq!(providers["codex"]["available_on_path"], true);
    assert_eq!(
        providers["codex"]["executable"].as_str().unwrap(),
        codex.to_string_lossy()
    );
    assert_eq!(providers["pi"]["available_on_path"], true);
    assert_eq!(
        providers["pi"]["executable"].as_str().unwrap(),
        pi.to_string_lossy()
    );
    assert_eq!(providers["claude"]["available_on_path"], false);
    assert_eq!(
        providers["claude"]["unavailable_reason"],
        "program_not_found"
    );
    assert_eq!(providers["claude"]["plan_mode"], true);
    assert_eq!(providers["codex"]["plan_mode"], false);
    assert_eq!(providers["grok"]["program"], "grok");
    assert_eq!(providers["grok"]["available_on_path"], false);
    assert_eq!(providers["grok"]["unavailable_reason"], "program_not_found");
    assert_eq!(result["pty_persistence"]["config_enabled"], false);
    assert_eq!(result["pty_persistence"]["available"], false);
    assert_eq!(result["pty_persistence"]["active"], false);
    assert_eq!(
        result["pty_persistence"]["unavailable_reason"],
        "broker_not_found"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn capabilities_report_pty_persistence_broker_status() {
    let dir = tempfile::tempdir().unwrap();
    let dtach = write_fake_program(dir.path(), "dtach");
    let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
    let config_home = tempfile::tempdir().unwrap();
    let forktty_config_dir = config_home.path().join("forktty");
    fs::create_dir_all(&forktty_config_dir).unwrap();
    fs::write(
        forktty_config_dir.join("config.toml"),
        r#"
            [general]
            persist_terminal_processes = true
            "#,
    )
    .unwrap();
    let _config_home = EnvGuard::set("XDG_CONFIG_HOME", config_home.path().to_str().unwrap());
    let (state, _backend) = test_state();

    let result = dispatch(&state, "system.capabilities", json!({}))
        .await
        .unwrap();

    let pty = &result["pty_persistence"];
    assert_eq!(pty["config_enabled"], true);
    assert_eq!(pty["available"], true);
    assert_eq!(pty["active"], true);
    assert_eq!(pty["broker"], "dtach");
    assert_eq!(
        pty["broker_executable"].as_str().unwrap(),
        dtach.to_string_lossy()
    );
    assert_eq!(pty["scope"], "plain_terminal_surfaces");
    assert_eq!(pty["unavailable_reason"], Value::Null);
}

#[tokio::test]
#[serial_test::serial]
async fn capabilities_report_configured_provider_command_outside_path() {
    let path_dir = tempfile::tempdir().unwrap();
    let custom_dir = tempfile::tempdir().unwrap();
    let custom_codex = write_fake_program(custom_dir.path(), "codex-custom");
    let _path = EnvGuard::set("PATH", path_dir.path().to_str().unwrap());
    let config_home = tempfile::tempdir().unwrap();
    let forktty_config_dir = config_home.path().join("forktty");
    fs::create_dir_all(&forktty_config_dir).unwrap();
    fs::write(
        forktty_config_dir.join("config.toml"),
        format!(
            r#"
            [general]
            shell = "/bin/sh"

            [team]
            provider_commands = {{ codex = "{}" }}
            "#,
            custom_codex.display()
        ),
    )
    .unwrap();
    let _config_home = EnvGuard::set("XDG_CONFIG_HOME", config_home.path().to_str().unwrap());
    let (state, _backend) = test_state();

    let result = dispatch(&state, "system.capabilities", json!({}))
        .await
        .unwrap();

    let custom_codex = custom_codex.to_string_lossy().into_owned();
    let provider = &result["provider_capabilities"]["codex"];
    assert_eq!(provider["launchable"], true);
    assert_eq!(provider["available_on_path"], false);
    assert_eq!(provider["program"].as_str().unwrap(), custom_codex);
    assert_eq!(provider["default_program"], "codex");
    assert_eq!(
        provider["configured_command"].as_str().unwrap(),
        custom_codex
    );
    assert_eq!(provider["executable"].as_str().unwrap(), custom_codex);
    assert_eq!(
        result["team_provider_policy"]["provider_commands"]["codex"]
            .as_str()
            .unwrap(),
        custom_codex
    );
}

#[tokio::test]
#[serial_test::serial]
async fn team_worker_launch_auto_selects_first_available_configured_provider() {
    let dir = tempfile::tempdir().unwrap();
    let pi = write_fake_program(dir.path(), "pi");
    let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
    let config_home = tempfile::tempdir().unwrap();
    let forktty_config_dir = config_home.path().join("forktty");
    fs::create_dir_all(&forktty_config_dir).unwrap();
    fs::write(
        forktty_config_dir.join("config.toml"),
        r#"
            [general]
            shell = "/bin/sh"

            [team]
            default_agent = "auto"
            provider_order = ["claude", "pi", "codex"]
            auto_fallback = true
            "#,
    )
    .unwrap();
    let _config_home = EnvGuard::set("XDG_CONFIG_HOME", config_home.path().to_str().unwrap());
    let (mut state, _backend) = test_state();
    let team_store = tempfile::tempdir().unwrap();
    state.team_store_path = Some(team_store.path().join("team-v1.json"));
    let main = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let main_workspace_id = main[0]["id"].as_str().unwrap().to_string();
    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": main_workspace_id,
            "name": "Launch",
        }),
    )
    .await
    .unwrap();

    let launched = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
            "role": "reviewer",
        }),
    )
    .await
    .unwrap();

    assert_eq!(launched["worker"]["agent"], "pi");
    assert_eq!(launched["argv"][0], "pi");
    assert_eq!(launched["selection"]["requested_agent"], "auto");
    assert_eq!(launched["selection"]["selected_agent"], "pi");
    assert_eq!(
        launched["selection"]["executable"].as_str().unwrap(),
        pi.to_string_lossy()
    );
    let first_considered = &launched["selection"]["considered"][0];
    assert_eq!(first_considered["agent"], "claude");
    assert_eq!(first_considered["program"], "claude");
    assert_eq!(first_considered["available"], false);
    assert_eq!(first_considered["reason"], "program_not_found");
}

#[tokio::test]
#[serial_test::serial]
async fn team_worker_launch_auto_uses_configured_command_outside_path() {
    let path_dir = tempfile::tempdir().unwrap();
    let custom_dir = tempfile::tempdir().unwrap();
    let custom_codex = write_fake_program(custom_dir.path(), "codex-custom");
    let _path = EnvGuard::set("PATH", path_dir.path().to_str().unwrap());
    let config_home = tempfile::tempdir().unwrap();
    let forktty_config_dir = config_home.path().join("forktty");
    fs::create_dir_all(&forktty_config_dir).unwrap();
    fs::write(
        forktty_config_dir.join("config.toml"),
        format!(
            r#"
            [general]
            shell = "/bin/sh"

            [team]
            default_agent = "auto"
            provider_order = ["codex"]
            provider_commands = {{ codex = "{}" }}
            "#,
            custom_codex.display()
        ),
    )
    .unwrap();
    let _config_home = EnvGuard::set("XDG_CONFIG_HOME", config_home.path().to_str().unwrap());
    let (mut state, backend) = test_state();
    let team_store = tempfile::tempdir().unwrap();
    state.team_store_path = Some(team_store.path().join("team-v1.json"));
    let main = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let main_workspace_id = main[0]["id"].as_str().unwrap().to_string();
    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": main_workspace_id,
            "name": "Launch",
        }),
    )
    .await
    .unwrap();

    let launched = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
        }),
    )
    .await
    .unwrap();

    let launched_surface_id = launched["surface"]["id"].as_str().unwrap();
    let custom_codex = custom_codex.to_string_lossy().into_owned();
    assert_eq!(launched["worker"]["agent"], "codex");
    assert_eq!(launched["argv"][0].as_str().unwrap(), custom_codex);
    assert_eq!(
        backend.spawn_shell(launched_surface_id).unwrap(),
        custom_codex
    );
    assert_eq!(
        launched["selection"]["program"].as_str().unwrap(),
        custom_codex
    );
    assert_eq!(launched["selection"]["default_program"], "codex");
    assert_eq!(
        launched["selection"]["configured_command"]
            .as_str()
            .unwrap(),
        custom_codex
    );
    assert_eq!(launched["selection"]["considered"][0]["available"], true);
}

#[tokio::test]
#[serial_test::serial]
async fn team_worker_launch_auto_rejects_when_no_provider_is_available() {
    let dir = tempfile::tempdir().unwrap();
    let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
    let config_home = tempfile::tempdir().unwrap();
    let _config_home = EnvGuard::set("XDG_CONFIG_HOME", config_home.path().to_str().unwrap());
    let (mut state, _backend) = test_state();
    let team_store = tempfile::tempdir().unwrap();
    state.team_store_path = Some(team_store.path().join("team-v1.json"));
    let main = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let main_workspace_id = main[0]["id"].as_str().unwrap().to_string();
    dispatch(
        &state,
        "team.upsert",
        json!({
            "team_id": "team-1",
            "workspace_id": main_workspace_id,
            "name": "Launch",
        }),
    )
    .await
    .unwrap();

    let err = dispatch(
        &state,
        "team.worker.launch",
        json!({
            "team_id": "team-1",
            "worker_id": "worker-1",
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err
        .to_string()
        .contains("No team worker provider is available"));
}

#[test]
fn team_worker_launch_accepts_documented_provider_aliases() {
    for (alias, expected_program) in [
        ("codex", "codex"),
        ("claude", "claude"),
        ("claude_code", "claude"),
        ("claude-code", "claude"),
        ("opencode", "opencode"),
        ("open_code", "opencode"),
        ("open-code", "opencode"),
        ("antigravity", "agy"),
        ("agy", "agy"),
        ("grok", "grok"),
        ("grok_build", "grok"),
        ("grok-build", "grok"),
        ("pi", "pi"),
    ] {
        let (program, args) = team_worker_launch_command(alias, None, Vec::new()).unwrap();
        assert_eq!(program, expected_program, "{alias}");
        if expected_program == "claude" {
            assert_eq!(
                args,
                vec!["--permission-mode".to_string(), "auto".to_string()],
                "{alias}"
            );
        } else {
            assert!(args.is_empty(), "{alias}");
        }
    }
}

#[test]
fn team_worker_launch_uses_dont_ask_for_claude_review_roles() {
    let (program, args) =
        team_worker_launch_command("claude-code", Some("code-reviewer"), Vec::new()).unwrap();

    assert_eq!(program, "claude");
    assert_eq!(
        &args[0..2],
        ["--permission-mode".to_string(), "dontAsk".to_string()]
    );
    let tools = args
        .windows(2)
        .find(|pair| pair[0] == "--allowedTools")
        .map(|pair| pair[1].as_str())
        .expect("review profile should set Claude allowed tools");
    assert_eq!(tools, CLAUDE_TEAM_REVIEW_ALLOWED_TOOLS);
    assert!(tools.contains("Read"));
    assert!(tools.contains("Grep"));
    assert!(tools.contains("Glob"));
    assert!(!tools.contains("Bash("));
    assert!(!tools.contains("cargo test"));
}

#[test]
fn team_worker_launch_uses_read_only_tools_for_pi_review_roles() {
    let (program, args) =
        team_worker_launch_command("pi", Some("code-reviewer"), Vec::new()).unwrap();

    assert_eq!(program, "pi");
    assert_eq!(args, ["--tools", "read,grep,find,ls"]);
}

#[test]
fn team_worker_launch_does_not_add_pi_tools_for_non_review_roles() {
    let (program, args) = team_worker_launch_command("pi", Some("builder"), Vec::new()).unwrap();

    assert_eq!(program, "pi");
    assert!(args.is_empty());
}

#[test]
fn team_worker_launch_preserves_explicit_pi_tool_args() {
    for args in [
        vec!["--tools".to_string(), "read".to_string()],
        vec!["--tools=read".to_string()],
        vec!["-t".to_string(), "read".to_string()],
        vec!["--exclude-tools=bash".to_string()],
        vec!["-xt".to_string(), "bash".to_string()],
        vec!["--no-tools".to_string()],
        vec!["-nt".to_string()],
        vec!["--no-builtin-tools".to_string()],
        vec!["-nbt".to_string()],
    ] {
        let (_, actual) = team_worker_launch_command("pi", Some("reviewer"), args.clone()).unwrap();
        assert_eq!(actual, args);
    }
}

#[test]
fn team_worker_launch_preserves_explicit_claude_permission_args() {
    let explicit_args = vec![
        "--permission-mode".to_string(),
        "default".to_string(),
        "--model".to_string(),
        "opus".to_string(),
    ];

    let (program, args) =
        team_worker_launch_command("claude", Some("reviewer"), explicit_args.clone()).unwrap();

    assert_eq!(program, "claude");
    assert_eq!(args, explicit_args);

    let equals_args = vec!["--permission-mode=default".to_string()];
    let (_, args) =
        team_worker_launch_command("claude", Some("reviewer"), equals_args.clone()).unwrap();
    assert_eq!(args, equals_args);
}

#[test]
fn team_worker_launch_preserves_each_claude_permission_override() {
    for args in [
        vec!["--allowedTools".to_string(), "Read".to_string()],
        vec!["--allowed-tools=Read".to_string()],
        vec!["--disallowedTools".to_string(), "Bash".to_string()],
        vec!["--disallowed-tools=Bash".to_string()],
        vec!["--settings".to_string(), "/tmp/settings.json".to_string()],
        vec!["--settings=/tmp/settings.json".to_string()],
        vec!["--dangerously-skip-permissions".to_string()],
        vec!["--allow-dangerously-skip-permissions".to_string()],
    ] {
        let (_, actual) =
            team_worker_launch_command("claude", Some("reviewer"), args.clone()).unwrap();
        assert_eq!(actual, args);
    }
}

#[test]
fn team_worker_launch_prepends_review_defaults_before_extra_args() {
    let extra = vec!["--model".to_string(), "opus".to_string()];
    let (_, args) =
        team_worker_launch_command("claude", Some("code-reviewer"), extra.clone()).unwrap();

    assert_eq!(
        &args[0..2],
        ["--permission-mode".to_string(), "dontAsk".to_string()]
    );
    assert_eq!(&args[args.len() - 2..], extra.as_slice());
}

#[test]
fn team_worker_launch_does_not_treat_preview_as_review_role() {
    let (_, args) = team_worker_launch_command("claude", Some("preview"), Vec::new()).unwrap();

    assert_eq!(
        args,
        vec!["--permission-mode".to_string(), "auto".to_string()]
    );
}
