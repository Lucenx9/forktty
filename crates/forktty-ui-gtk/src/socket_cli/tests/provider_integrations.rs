use super::*;

#[test]
fn antigravity_setup_plan_writes_named_group_and_wrapper_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().display().to_string();
    with_env(&[("HOME", Some(home.as_str()))], || {
        let spec = agent_spec("antigravity").unwrap();
        let plan = build_hook_setup_plan(spec, Path::new("/usr/bin/forktty")).unwrap();
        assert!(plan.changed);
        assert_eq!(plan.config_path, antigravity_config_path());

        let config: Value = serde_json::from_str(&plan.content).unwrap();
        let group = &config[ANTIGRAVITY_HOOK_GROUP];
        assert_eq!(group["enabled"], Value::Bool(true));
        // Model lifecycle hooks use flat handlers; tool events use
        // matcher wrappers.
        assert_eq!(
            group["PreInvocation"][0]["command"],
            antigravity_script_path("before-model")
                .display()
                .to_string()
        );
        assert!(group["PreInvocation"][0].get("hooks").is_none());
        assert!(group["PreInvocation"][0].get("matcher").is_none());
        assert_eq!(group["PreToolUse"][0]["matcher"], json!("*"));
        assert_eq!(group["PostToolUse"][0]["matcher"], json!("*"));

        assert_eq!(plan.scripts.len(), 3);
        for (event, provider_event) in [
            ("before-model", "PreInvocation"),
            ("pre-tool", "PreToolUse"),
            ("post-tool", "PostToolUse"),
        ] {
            let script_path = antigravity_script_path(event);
            let command = if provider_event == "PreInvocation" {
                group[provider_event][0]["command"].as_str().unwrap()
            } else {
                group[provider_event][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap()
            };
            assert_eq!(command, script_path.display().to_string());
            let (_, content) = plan
                .scripts
                .iter()
                .find(|(path, _)| path == &script_path)
                .unwrap();
            assert!(content.starts_with("#!/bin/sh\n"));
            assert!(content.contains(ANTIGRAVITY_SCRIPT_TAG));
            assert!(content.contains(&format!("'/usr/bin/forktty' hooks antigravity {event}")));
            assert!(content.contains("FORKTTY_ANTIGRAVITY_HOOKS_DISABLED"));
        }
    });
}

#[test]
fn antigravity_appimage_wrappers_use_extract_and_run_env() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().display().to_string();
    with_env(&[("HOME", Some(home.as_str()))], || {
        let spec = agent_spec("antigravity").unwrap();
        let plan =
            build_hook_setup_plan(spec, Path::new("/home/me/AppImages/forktty.appimage")).unwrap();

        let (_, content) = plan
            .scripts
            .iter()
            .find(|(path, _)| path == &antigravity_script_path("before-model"))
            .unwrap();
        assert!(content.contains(
            "APPIMAGE_EXTRACT_AND_RUN=1 '/home/me/AppImages/forktty.appimage' hooks antigravity before-model"
        ));
    });
}

#[test]
fn antigravity_setup_preserves_foreign_groups_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().display().to_string();
    with_env(&[("HOME", Some(home.as_str()))], || {
        let spec = agent_spec("antigravity").unwrap();
        let config_path = antigravity_config_path();
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::write(
            &config_path,
            r#"{"safety-gate":{"enabled":true,"PreToolUse":[{"matcher":"run_command","hooks":[{"type":"command","command":"/usr/local/bin/guard.sh"}]}]}}"#,
        )
        .unwrap();

        let plan = build_hook_setup_plan(spec, Path::new("/usr/bin/forktty")).unwrap();
        assert!(plan.changed);
        let config: Value = serde_json::from_str(&plan.content).unwrap();
        assert_eq!(
            config["safety-gate"]["PreToolUse"][0]["hooks"][0]["command"],
            json!("/usr/local/bin/guard.sh")
        );
        assert!(config[ANTIGRAVITY_HOOK_GROUP].is_object());

        // Simulate a full setup run, then re-plan: nothing changes.
        fs::write(&config_path, &plan.content).unwrap();
        for (script_path, content) in &plan.scripts {
            fs::create_dir_all(script_path.parent().unwrap()).unwrap();
            fs::write(script_path, content).unwrap();
            fs::set_permissions(script_path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let replanned = build_hook_setup_plan(spec, Path::new("/usr/bin/forktty")).unwrap();
        assert!(!replanned.changed);
    });
}

#[test]
fn antigravity_setup_hardens_config_and_wrapper_directories() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().display().to_string();
    with_env(&[("HOME", Some(home.as_str()))], || {
        let root_dir = antigravity_root_dir();
        let config_dir = antigravity_config_dir();
        let scripts_dir = antigravity_scripts_dir();
        fs::create_dir_all(&scripts_dir).unwrap();
        fs::set_permissions(&root_dir, fs::Permissions::from_mode(0o777)).unwrap();
        fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o777)).unwrap();
        fs::set_permissions(&scripts_dir, fs::Permissions::from_mode(0o777)).unwrap();

        handle_hooks_setup(&test_context(), strings(&["antigravity"])).unwrap();

        assert_eq!(
            fs::metadata(&root_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&config_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&scripts_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for event in ["before-model", "pre-tool", "post-tool"] {
            let script_path = antigravity_script_path(event);
            assert_eq!(
                fs::metadata(script_path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    });
}

#[test]
fn antigravity_setup_rejects_symlinked_hook_directories() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let target = dir.path().join("target");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o777)).unwrap();
    std::os::unix::fs::symlink(&target, home.join(".gemini")).unwrap();
    let home = home.display().to_string();

    with_env(&[("HOME", Some(home.as_str()))], || {
        let err = handle_hooks_setup(&test_context(), strings(&["antigravity"]))
            .expect_err("symlinked Antigravity hook root must be rejected");
        assert!(err.message.contains("refusing symlinked hook directory"));
    });

    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o777
    );
}

#[test]
fn antigravity_remove_plan_deletes_group_scripts_and_solely_owned_file() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().display().to_string();
    with_env(&[("HOME", Some(home.as_str()))], || {
        let spec = agent_spec("antigravity").unwrap();
        let config_path = antigravity_config_path();
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let plan = build_hook_setup_plan(spec, Path::new("/usr/bin/forktty")).unwrap();
        fs::write(&config_path, &plan.content).unwrap();
        for (script_path, content) in &plan.scripts {
            fs::create_dir_all(script_path.parent().unwrap()).unwrap();
            fs::write(script_path, content).unwrap();
        }

        let remove = build_hook_remove_plan(spec, Some(Path::new("/usr/bin/forktty"))).unwrap();
        assert!(remove.changed);
        assert_eq!(remove.scripts_dir, Some(antigravity_scripts_dir()));
        assert!(matches!(remove.action, HookRemoveAction::DeleteFile));

        // With a foreign group present, only the forktty group is removed.
        let mut config: Value = serde_json::from_str(&plan.content).unwrap();
        config["safety-gate"] = json!({"enabled": true});
        fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();
        let remove = build_hook_remove_plan(spec, Some(Path::new("/usr/bin/forktty"))).unwrap();
        assert!(remove.changed);
        match &remove.action {
            HookRemoveAction::Write(content) => {
                let next: Value = serde_json::from_str(content).unwrap();
                assert!(next.get(ANTIGRAVITY_HOOK_GROUP).is_none());
                assert!(next["safety-gate"].is_object());
            }
            _ => panic!("expected a rewrite that keeps the foreign group"),
        }

        // Nothing installed: nothing to do.
        fs::remove_file(&config_path).unwrap();
        fs::remove_dir_all(antigravity_scripts_dir()).unwrap();
        let remove = build_hook_remove_plan(spec, Some(Path::new("/usr/bin/forktty"))).unwrap();
        assert!(!remove.changed);
    });
}

#[test]
fn antigravity_launcher_check_reads_wrapper_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().display().to_string();
    with_env(&[("HOME", Some(home.as_str()))], || {
        let spec = agent_spec("antigravity").unwrap();
        let config_path = antigravity_config_path();
        let check =
            describe_launcher_check(spec, &config_path, Some(Path::new("/usr/bin/forktty")));
        assert_eq!(check["status"], json!("not_installed"));

        let plan = build_hook_setup_plan(spec, Path::new("/old/forktty")).unwrap();
        for (script_path, content) in &plan.scripts {
            fs::create_dir_all(script_path.parent().unwrap()).unwrap();
            fs::write(script_path, content).unwrap();
        }
        let check = describe_launcher_check(spec, &config_path, Some(Path::new("/new/forktty")));
        assert_eq!(check["status"], json!("stale"));
        assert_eq!(check["installedLauncher"], json!("/old/forktty"));
    });
}

#[test]
fn antigravity_session_id_comes_from_conversation_id() {
    assert_eq!(
        extract_hook_session_id(&json!({"conversationId": "032316f4-2fae"})),
        Some("032316f4-2fae".to_string())
    );
}

#[test]
fn codex_trust_report_classifies_recorded_and_unrecorded_events() {
    let hooks_json = Path::new("/home/me/.codex/hooks.json");
    let config_toml = Path::new("/home/me/.codex/config.toml");
    let state: toml::Table = r#"
        ["/home/me/.codex/hooks.json:pre_tool_use:0:0"]
        trusted_hash = "sha256:abc"
        ["/other/hooks.json:stop:0:0"]
        trusted_hash = "sha256:def"
    "#
    .parse()
    .unwrap();
    let report = codex_hook_trust_report(config_toml, hooks_json, CODEX_HOOK_ENTRIES, Some(&state));
    assert_eq!(report["status"], json!("partial"));
    assert_eq!(report["currentHashesVerified"], json!(false));
    assert!(report["hint"].as_str().unwrap().contains("current hash"));
    assert_eq!(report["recordedEvents"], json!(["PreToolUse"]));
    assert!(report["unrecordedEvents"]
        .as_array()
        .unwrap()
        .contains(&json!("Stop")));

    let report = codex_hook_trust_report(config_toml, hooks_json, CODEX_HOOK_ENTRIES, None);
    assert_eq!(report["status"], json!("none_recorded"));
}

#[test]
fn codex_trust_summary_never_claims_current_hashes_are_verified() {
    let report = json!({
        "status": "all_recorded",
        "recordedEvents": CODEX_HOOK_ENTRIES
            .iter()
            .map(|entry| entry.event_name)
            .collect::<Vec<_>>(),
        "unrecordedEvents": [],
        "currentHashesVerified": false,
    });

    let summary = format_codex_trust_check(&report).unwrap();
    assert!(summary.contains("cannot verify"));
    assert!(summary.contains("/hooks"));
}

#[test]
fn hook_setup_command_uses_forktty_launcher_without_node() {
    let spec = agent_spec("codex").unwrap();
    let command = build_hook_shell_command(
        Path::new("/home/me/ForkTTY/forktty.AppImage"),
        spec,
        "session-start",
    );
    assert!(command.contains("'/home/me/ForkTTY/forktty.AppImage' hooks codex session-start"));
    assert!(!command.contains("node"));
    assert!(command.contains("FORKTTY_CODEX_HOOKS_DISABLED"));
}

#[test]
fn merge_hook_config_strips_legacy_script_entry() {
    let spec = agent_spec("codex").unwrap();
    let existing = json!({
        "hooks": {
            "SessionStart": [{
                "hooks": [{
                    "type": "command",
                    "command": "[ \"${FORKTTY_CODEX_HOOKS_DISABLED:-}\" != \"1\" ] && node '/old/scripts/forktty.mjs' hooks codex session-start || echo '{\"continue\":true,\"suppressOutput\":false}'",
                    "timeout": 5000
                }]
            }]
        },
        "custom": true
    });
    let (_, merged) = merge_hook_config(&existing, spec, Path::new("/usr/bin/forktty")).unwrap();
    let entries = merged["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    let command = entries[0]["hooks"][0]["command"].as_str().unwrap();
    assert!(command.contains("'/usr/bin/forktty' hooks codex session-start"));
    assert!(!command.contains("forktty.mjs"));
    assert_eq!(merged["custom"], Value::Bool(true));
}

#[test]
fn claude_hook_setup_removes_retired_worktree_hooks_without_touching_user_hooks() {
    let spec = agent_spec("claude").unwrap();
    let user_entry = json!({
        "hooks": [{
            "type": "command",
            "command": "user-worktree-provider"
        }]
    });
    let existing = json!({
        "hooks": {
            "WorktreeCreate": [
                {
                    "hooks": [{
                        "type": "command",
                        "command": "stale-tagged-command"
                    }],
                    "forkttySource": FORKTTY_HOOK_TAG
                },
                {
                    "hooks": [{
                        "type": "command",
                        "command": "[ \"${FORKTTY_CLAUDE_HOOKS_DISABLED:-}\" != \"1\" ] && '/old/forktty' hooks claude worktree-create || echo '{}'"
                    }]
                },
                user_entry.clone()
            ]
        }
    });

    let (changed, merged) = merge_hook_config_with_profile(
        &existing,
        spec,
        Path::new("/usr/bin/forktty"),
        HookSetupProfile::Lifecycle,
    )
    .unwrap();

    assert!(changed);
    assert_eq!(merged["hooks"]["WorktreeCreate"], json!([user_entry]));
}

#[test]
fn claude_hook_remove_cleans_retired_worktree_hooks() {
    let spec = agent_spec("claude").unwrap();
    let existing = json!({
        "hooks": {
            "WorktreeCreate": [
                {
                    "hooks": [{
                        "type": "command",
                        "command": "stale-tagged-command"
                    }],
                    "forkttySource": FORKTTY_HOOK_TAG
                },
                {
                    "hooks": [{
                        "type": "command",
                        "command": "[ \"${FORKTTY_CLAUDE_HOOKS_DISABLED:-}\" != \"1\" ] && '/old/forktty' hooks claude worktree-create || echo '{}'"
                    }]
                },
                {
                    "hooks": [{
                        "type": "command",
                        "command": "user-worktree-provider"
                    }]
                }
            ]
        }
    });

    let (changed, removed) =
        remove_hook_config(&existing, spec, Some(Path::new("/usr/bin/forktty"))).unwrap();

    assert!(changed);
    assert_eq!(
        removed["hooks"]["WorktreeCreate"],
        json!([{
            "hooks": [{
                "type": "command",
                "command": "user-worktree-provider"
            }]
        }])
    );
}

#[test]
fn merge_hook_config_installs_current_codex_observability_events() {
    let (_, codex) = merge_hook_config(
        &json!({}),
        agent_spec("codex").unwrap(),
        Path::new("/usr/bin/forktty"),
    )
    .unwrap();
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PermissionRequest",
        "PreCompact",
        "PostCompact",
        "SubagentStart",
        "SubagentStop",
        "Stop",
    ] {
        assert!(codex["hooks"][event].is_array(), "missing Codex {event}");
    }
    assert!(codex["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("hooks codex pre-tool"));
}

#[test]
fn hook_templates_match_native_installer_specs() {
    for (agent, template) in [
        ("codex", "codex-hooks.json"),
        ("claude", "claude-settings.json"),
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("hooks")
            .join(template);
        let template_json: Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let spec = agent_spec(agent).unwrap();
        let profile = if agent == "claude" {
            HookSetupProfile::Lifecycle
        } else {
            HookSetupProfile::Full
        };
        let (_, generated) = merge_hook_config_with_profile(
            &json!({}),
            spec,
            Path::new("{{FORKTTY_LAUNCHER}}"),
            profile,
        )
        .unwrap();
        assert_eq!(
            template_json,
            generated_without_installer_tags(generated),
            "{template} is out of sync with the native hook installer"
        );
    }
}

#[test]
fn opencode_plugin_plan_is_idempotent_and_protects_unmanaged_files() {
    let dir = tempfile::tempdir().unwrap();
    let home_s = dir.path().display().to_string();
    with_env(
        &[("HOME", Some(&home_s)), ("OPENCODE_CONFIG_DIR", None)],
        || {
            let spec = agent_spec("opencode").unwrap();
            let launcher = Path::new("/usr/bin/forktty");
            let first = build_hook_setup_plan(spec, launcher).unwrap();
            assert!(first.changed);
            assert!(first.content.contains(OPENCODE_PLUGIN_TAG));
            assert!(!first.content.contains("pub(in crate::socket_cli)"));
            assert!(first.content.contains("const HOOK_TIMEOUT_MS = 30000;"));
            assert!(first.content.contains("timeout: HOOK_TIMEOUT_MS,"));
            assert_eq!(
                extract_launcher_from_opencode_plugin(&first.content).as_deref(),
                Some("/usr/bin/forktty")
            );

            let appimage =
                build_hook_setup_plan(spec, Path::new("/home/me/AppImages/forktty.appimage"))
                    .unwrap();
            assert!(appimage
                .content
                .contains("const APPIMAGE_EXTRACT_AND_RUN = true;"));
            assert!(appimage
                .content
                .contains(r#""APPIMAGE_EXTRACT_AND_RUN": "1""#));

            ensure_parent_dir(&first.config_path).unwrap();
            atomic_write_file(&first.config_path, first.content.as_bytes()).unwrap();
            let second = build_hook_setup_plan(spec, launcher).unwrap();
            assert!(!second.changed);

            atomic_write_file(
                &first.config_path,
                b"export const Mine = async () => ({})\n",
            )
            .unwrap();
            assert_err_contains(
                build_hook_setup_plan(spec, launcher),
                "refusing to overwrite unmanaged plugin file",
            );
        },
    );
}

fn generated_without_installer_tags(mut value: Value) -> Value {
    if let Some(hooks) = value.get_mut("hooks").and_then(Value::as_object_mut) {
        for entries in hooks.values_mut().filter_map(Value::as_array_mut) {
            for entry in entries {
                if let Some(object) = entry.as_object_mut() {
                    object.remove("forkttySource");
                }
            }
        }
    }
    value
}

#[test]
fn merge_hook_config_preserves_unrelated_hook_commands() {
    let spec = agent_spec("codex").unwrap();
    let existing = json!({
        "hooks": {
            "SessionStart": [{
                "hooks": [{
                    "type": "command",
                    "command": "custom-wrapper hooks codex session-start"
                }]
            }]
        }
    });
    let (_, merged) = merge_hook_config(&existing, spec, Path::new("/usr/bin/forktty")).unwrap();
    let entries = merged["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries[0]["hooks"][0]["command"],
        Value::String("custom-wrapper hooks codex session-start".to_string())
    );
}

#[test]
fn codex_and_claude_hook_timeouts_are_seconds_within_provider_budget() {
    // Codex docs treat `timeout` as seconds (default 600). Claude Code
    // hooks reference also documents seconds (default 600, 30 for
    // UserPromptSubmit). The installer must emit a value that is
    // measured in seconds and stays under the smaller of the two
    // provider defaults so we never block the agent loop longer than a
    // local round-trip needs.
    assert_eq!(HOOK_ENTRY_TIMEOUT_SECS, 30);
    for spec in [agent_spec("codex").unwrap(), agent_spec("claude").unwrap()] {
        let (_, config) =
            merge_hook_config(&json!({}), spec, Path::new("/usr/bin/forktty")).unwrap();
        let hooks = config["hooks"].as_object().expect("hooks object");
        for entries in hooks.values() {
            for entry in entries.as_array().expect("entry array") {
                if !is_forktty_managed_entry(entry) {
                    continue;
                }
                for hook in entry["hooks"].as_array().expect("hooks array") {
                    let timeout = hook["timeout"].as_u64().expect("integer timeout");
                    assert_eq!(
                        timeout, HOOK_ENTRY_TIMEOUT_SECS,
                        "{} entry must encode timeout in seconds",
                        spec.key
                    );
                    assert!(
                        timeout < 600,
                        "{} timeout must stay under provider default",
                        spec.key
                    );
                }
            }
        }
    }
}
