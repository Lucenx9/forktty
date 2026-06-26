use super::*;

#[test]
fn hook_setup_writes_all_agent_configs_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex home");
    let claude_dir = dir.path().join("claude config");
    let home = dir.path().join("home dir");
    let codex_home_s = codex_home.display().to_string();
    let claude_dir_s = claude_dir.display().to_string();
    let home_s = home.display().to_string();

    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
            ("HOME", Some(&home_s)),
        ],
        || {
            let context = test_context();
            handle_hooks_setup(&context, strings(&["codex", "claude", "opencode", "codex"]))
                .unwrap();

            let codex_path = codex_home.join("hooks.json");
            let claude_path = claude_dir.join("settings.json");
            let opencode_path = home
                .join(".config/opencode")
                .join("plugins/forktty.generated.js");
            let codex = read_json(&codex_path);
            assert!(codex["hooks"]["SessionStart"].is_array());
            assert!(codex["hooks"]["PermissionRequest"].is_array());
            assert!(codex["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains(" hooks codex pre-tool"));
            assert!(!codex["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("forktty.mjs"));

            let claude = read_json(&claude_path);
            for event in [
                "PermissionRequest",
                "SubagentStart",
                "SubagentStop",
                "PreCompact",
                "PostCompact",
                "StopFailure",
                "SessionEnd",
            ] {
                assert!(claude["hooks"][event].is_array(), "missing {event}");
            }
            for event in [
                "PreToolUse",
                "PostToolUse",
                "PostToolUseFailure",
                "PostToolBatch",
            ] {
                assert!(
                    claude["hooks"].get(event).is_none(),
                    "default Claude setup should omit {event}"
                );
            }
            assert!(
                claude["hooks"]["PermissionRequest"][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap()
                    .contains(" hooks claude permission-request")
            );
            assert_eq!(describe_claude_installed_profile(&claude_path), "lifecycle");

            assert!(!home.join(".gemini/settings.json").exists());
            let opencode = fs::read_to_string(&opencode_path).unwrap();
            assert!(opencode.contains(OPENCODE_PLUGIN_TAG));
            assert!(opencode.contains("hooks\", \"opencode\""));
            assert!(opencode.contains("\"tool.execute.before\""));
            assert!(opencode.contains("const MAX_INPUT_BYTES = 1048576"));
            assert!(opencode.contains("const MAX_SANITIZE_NODES = 4096"));
            assert!(opencode.contains("function makeBudget"));
            assert!(opencode.contains("function sanitizeJson"));
            assert!(opencode.contains("input: hookInput(body)"));

            let first = fs::read_to_string(&codex_path).unwrap();
            handle_hooks_setup(&context, strings(&["codex"])).unwrap();
            assert_eq!(fs::read_to_string(&codex_path).unwrap(), first);
            assert_eq!(backup_count(&codex_home, "hooks.json.bak-"), 0);
        },
    );
}

#[test]
fn hook_setup_rejects_removed_gemini_target() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex home");
    let claude_dir = dir.path().join("claude config");
    let home = dir.path().join("home dir");
    let codex_home_s = codex_home.display().to_string();
    let claude_dir_s = claude_dir.display().to_string();
    let home_s = home.display().to_string();

    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
            ("HOME", Some(&home_s)),
        ],
        || {
            let context = test_context();
            handle_hooks_setup(&context, Vec::new()).unwrap();

            assert!(codex_home.join("hooks.json").exists());
            assert!(claude_dir.join("settings.json").exists());
            assert!(home
                .join(".config/opencode")
                .join("plugins/forktty.generated.js")
                .exists());
            assert!(home.join(".gemini/config/hooks.json").exists());
            assert!(!home.join(".gemini/settings.json").exists());

            let err = handle_hooks_setup(&context, strings(&["gemini"])).unwrap_err();
            assert!(err.message.contains("Unsupported agent: gemini"), "{err:?}");
            assert!(!home.join(".gemini/settings.json").exists());
        },
    );
}

#[test]
fn hook_remove_cleans_legacy_gemini_config_without_enabling_setup() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let context = test_context();
        let gemini_path = home.path().join(".gemini/settings.json");
        ensure_parent_dir(&gemini_path).unwrap();
        fs::write(
            &gemini_path,
            serde_json::to_string_pretty(&json!({
                "hooks": {
                    "SessionStart": [
                        {
                            "hooks": [{
                                "type": "command",
                                "command": "[ \"${FORKTTY_GEMINI_HOOKS_DISABLED:-}\" != \"1\" ] && '/usr/bin/forktty' hooks gemini session-start || echo '{\"continue\":true,\"suppressOutput\":false}'"
                            }]
                        },
                        {
                            "hooks": [{
                                "type": "command",
                                "command": "custom-gemini-hook"
                            }]
                        }
                    ]
                },
                "mcpServers": {
                    "forktty": {
                        "command": "/usr/bin/forktty",
                        "args": ["mcp"],
                        "env": { MCP_MANAGED_ENV: MCP_SERVER_NAME }
                    }
                },
                "theme": "dark"
            }))
            .unwrap(),
        )
        .unwrap();

        assert_err_contains(
            handle_hooks_setup(&context, strings(&["gemini"])),
            "Unsupported agent: gemini",
        );
        handle_hooks_remove(&context, strings(&["gemini"])).unwrap();

        let config = read_json(&gemini_path);
        let entries = config["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0]["hooks"][0]["command"],
            Value::String("custom-gemini-hook".to_string())
        );
        assert!(config["mcpServers"].get("forktty").is_some());
        assert_eq!(config["theme"], "dark");
    });
}

#[test]
fn claude_hook_setup_profiles_migrate_and_remove() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join("claude config");
    let home = dir.path().join("home dir");
    let claude_dir_s = claude_dir.display().to_string();
    let home_s = home.display().to_string();

    with_env(
        &[
            ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
            ("HOME", Some(&home_s)),
        ],
        || {
            let context = test_context();
            let claude_path = claude_dir.join("settings.json");

            handle_hooks_setup(&context, strings(&["claude"])).unwrap();
            let lifecycle = read_json(&claude_path);
            assert!(lifecycle["hooks"]["SessionStart"].is_array());
            assert!(lifecycle["hooks"]["PermissionRequest"].is_array());
            assert!(lifecycle["hooks"].get("PreToolUse").is_none());
            assert_eq!(describe_claude_installed_profile(&claude_path), "lifecycle");

            handle_hooks_setup(&context, strings(&["--full", "claude"])).unwrap();
            let full = read_json(&claude_path);
            for event in [
                "PreToolUse",
                "PostToolUse",
                "PostToolUseFailure",
                "PostToolBatch",
            ] {
                assert!(full["hooks"][event].is_array(), "missing full {event}");
            }
            assert!(full["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains(" hooks claude pre-tool"));
            assert_eq!(describe_claude_installed_profile(&claude_path), "full");

            handle_hooks_setup(&context, strings(&["claude"])).unwrap();
            let migrated = read_json(&claude_path);
            assert!(migrated["hooks"]["SessionStart"].is_array());
            assert!(migrated["hooks"]["PermissionRequest"].is_array());
            for event in [
                "PreToolUse",
                "PostToolUse",
                "PostToolUseFailure",
                "PostToolBatch",
            ] {
                assert!(
                    migrated["hooks"].get(event).is_none(),
                    "default rerun should remove {event}"
                );
            }
            assert_eq!(describe_claude_installed_profile(&claude_path), "lifecycle");

            handle_hooks_setup(&context, strings(&["--full", "claude"])).unwrap();
            handle_hooks_remove(&context, strings(&["claude"])).unwrap();
            let removed = read_json(&claude_path);
            assert!(removed.get("hooks").is_none());
            assert_eq!(
                describe_claude_installed_profile(&claude_path),
                "not_installed"
            );
        },
    );
}

#[test]
fn claude_hook_setup_plan_profiles_control_tool_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join("claude config");
    let home = dir.path().join("home dir");
    let claude_dir_s = claude_dir.display().to_string();
    let home_s = home.display().to_string();

    with_env(
        &[
            ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
            ("HOME", Some(&home_s)),
        ],
        || {
            let spec = agent_spec("claude").unwrap();
            let launcher = Path::new("/usr/bin/forktty");
            let default_plan = build_hook_setup_plan(spec, launcher).unwrap();
            let default_config: Value = serde_json::from_str(&default_plan.content).unwrap();
            assert!(default_config["hooks"]["SessionStart"].is_array());
            assert!(default_config["hooks"].get("PreToolUse").is_none());

            let full_plan =
                build_hook_setup_plan_with_profile(spec, launcher, HookSetupProfile::Full).unwrap();
            let full_config: Value = serde_json::from_str(&full_plan.content).unwrap();
            for event in [
                "PreToolUse",
                "PostToolUse",
                "PostToolUseFailure",
                "PostToolBatch",
            ] {
                assert!(full_config["hooks"][event].is_array(), "missing {event}");
            }
        },
    );
}

#[test]
fn hook_remove_deletes_only_forktty_managed_entries_and_plugins() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex");
    let home_s = dir.path().display().to_string();
    let codex_home_s = codex_home.display().to_string();
    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("HOME", Some(&home_s)),
            ("OPENCODE_CONFIG_DIR", None),
        ],
        || {
            let context = test_context();
            handle_hooks_setup(&context, strings(&["codex", "opencode"])).unwrap();
            let codex_path = codex_home.join("hooks.json");
            let opencode_path = dir
                .path()
                .join(".config/opencode")
                .join("plugins/forktty.generated.js");
            let mut codex = read_json(&codex_path);
            codex["hooks"]["SessionStart"] = json!([
                {
                    "hooks": [{
                        "type": "command",
                        "command": "custom-wrapper hooks codex session-start"
                    }]
                },
                codex["hooks"]["SessionStart"][0].clone()
            ]);
            atomic_write_file(
                &codex_path,
                format!("{}\n", serde_json::to_string_pretty(&codex).unwrap()).as_bytes(),
            )
            .unwrap();

            handle_hooks_remove(&context, strings(&["codex", "opencode"])).unwrap();

            let codex = read_json(&codex_path);
            let entries = codex["hooks"]["SessionStart"].as_array().unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(
                entries[0]["hooks"][0]["command"],
                Value::String("custom-wrapper hooks codex session-start".to_string())
            );
            assert!(codex["hooks"].get("PreToolUse").is_none());
            assert!(!opencode_path.exists());

            handle_hooks_remove(&context, strings(&["codex", "opencode"])).unwrap();
            assert!(opencode_path
                .parent()
                .unwrap()
                .read_dir()
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".bak-")));
        },
    );
}

#[test]
fn hook_remove_dry_run_and_option_errors_do_not_write_configs() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex");
    let codex_home_s = codex_home.display().to_string();
    let home_s = dir.path().display().to_string();
    with_env(
        &[("CODEX_HOME", Some(&codex_home_s)), ("HOME", Some(&home_s))],
        || {
            let context = test_context();
            handle_hooks_setup(&context, strings(&["codex"])).unwrap();
            let codex_path = codex_home.join("hooks.json");
            let before = fs::read_to_string(&codex_path).unwrap();

            handle_hooks_remove(&context, strings(&["--dry-run", "codex"])).unwrap();
            assert_eq!(fs::read_to_string(&codex_path).unwrap(), before);

            assert_err_contains(
                handle_hooks_remove(&context, strings(&["--dry-run=yes", "codex"])),
                "hooks remove: --dry-run must be true or false",
            );
            assert_err_contains(
                handle_hooks_remove(&context, strings(&["--dryrun", "codex"])),
                "hooks remove: unknown option --dryrun",
            );
        },
    );
}
