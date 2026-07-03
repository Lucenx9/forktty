use super::*;

#[test]
fn mcp_tools_default_targets_from_forktty_env() {
    with_env(
        &[
            ("FORKTTY_WORKSPACE_ID", Some(" ws-env ")),
            ("FORKTTY_SURFACE_ID", Some(" surface-env ")),
        ],
        || {
            let (_, params) = crate::mcp_server::build_socket_call_for_test(
                "surface_send_text",
                json!({ "text": "cargo test\n" }),
            )
            .unwrap();
            assert_eq!(params["surface_id"], "surface-env");
            assert_eq!(params["text"], "cargo test\n");

            let (_, params) =
                crate::mcp_server::build_socket_call_for_test("surface_read_text", json!({}))
                    .unwrap();
            assert_eq!(params["surface_id"], "surface-env");

            let (_, params) = crate::mcp_server::build_socket_call_for_test(
                "surface_capture_tail",
                json!({ "lines": 20 }),
            )
            .unwrap();
            assert_eq!(params["surface_id"], "surface-env");
            assert_eq!(params["lines"], 20);

            let (_, params) = crate::mcp_server::build_socket_call_for_test(
                "status_set",
                json!({ "key": "agent:codex", "value": "Running" }),
            )
            .unwrap();
            assert_eq!(params["workspace_id"], "ws-env");
            assert_eq!(params["surface_id"], "surface-env");
            assert_eq!(params["label"], "agent:codex");

            let (_, params) =
                crate::mcp_server::build_socket_call_for_test("surface_list", json!({})).unwrap();
            assert_eq!(params["workspace_id"], "ws-env");

            let (_, params) = crate::mcp_server::build_socket_call_for_test(
                "notification_create",
                json!({ "workspace_id": "explicit-ws", "body": "done" }),
            )
            .unwrap();
            assert_eq!(params["workspace_id"], "explicit-ws");
            assert!(params.get("surface_id").is_none());
        },
    );
}

#[test]
fn mcp_setup_plans_write_agent_configs_and_are_idempotent() {
    let home = tempfile::tempdir().unwrap();
    let codex_home = tempfile::tempdir().unwrap();
    let home = home.path().to_string_lossy().to_string();
    let codex_home = codex_home.path().to_string_lossy().to_string();
    with_env(
        &[
            ("HOME", Some(home.as_str())),
            ("CODEX_HOME", Some(codex_home.as_str())),
        ],
        || {
            let launcher = Path::new("/usr/bin/forktty");
            for agent in ["codex", "claude", "antigravity"] {
                let spec = mcp_agent_spec(agent).unwrap();
                let plan = build_mcp_setup_plan(spec, launcher).unwrap();
                assert!(plan.changed, "{agent} initial plan should change");
                ensure_parent_dir(&plan.config_path).unwrap();
                fs::write(&plan.config_path, &plan.content).unwrap();
                let replanned = build_mcp_setup_plan(spec, launcher).unwrap();
                assert!(!replanned.changed, "{agent} setup should be idempotent");
            }

            let codex: toml::Table = fs::read_to_string(codex_mcp_config_path())
                .unwrap()
                .parse()
                .unwrap();
            let codex_server = &codex["mcp_servers"]["forktty"];
            assert_eq!(codex_server["command"].as_str(), Some("/usr/bin/forktty"));
            assert_eq!(
                codex_server["args"].as_array().unwrap()[0].as_str(),
                Some("mcp")
            );
            assert_eq!(
                codex_server["env"][MCP_MANAGED_ENV].as_str(),
                Some(MCP_SERVER_NAME)
            );
            assert!(codex_server["env_vars"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value.as_str() == Some("FORKTTY_SOCKET_PATH")));

            for (agent, path) in [
                ("claude", claude_mcp_config_path()),
                ("antigravity", antigravity_mcp_config_path()),
            ] {
                let config = read_json(&path);
                let server = &config["mcpServers"]["forktty"];
                assert_eq!(server["command"], "/usr/bin/forktty", "{agent}");
                assert_eq!(server["args"][0], "mcp", "{agent}");
                assert_eq!(server["env"][MCP_MANAGED_ENV], MCP_SERVER_NAME, "{agent}");
            }
        },
    );
}

#[test]
fn mcp_setup_rejects_removed_gemini_target() {
    let home = tempfile::tempdir().unwrap();
    let codex_home = tempfile::tempdir().unwrap();
    let home = home.path().to_string_lossy().to_string();
    let codex_home = codex_home.path().to_string_lossy().to_string();
    with_env(
        &[
            ("HOME", Some(home.as_str())),
            ("CODEX_HOME", Some(codex_home.as_str())),
        ],
        || {
            let context = test_context();
            handle_mcp_setup(&context, Vec::new()).unwrap();

            assert!(codex_mcp_config_path().exists());
            assert!(claude_mcp_config_path().exists());
            assert!(antigravity_mcp_config_path().exists());
            let legacy_gemini_path = Path::new(&home).join(".gemini/settings.json");
            assert!(!legacy_gemini_path.exists());

            let err = handle_mcp_setup(&context, strings(&["gemini"])).unwrap_err();
            assert!(
                err.message.contains("Unsupported mcp agent: gemini"),
                "{err:?}"
            );
            assert!(!legacy_gemini_path.exists());
        },
    );
}

#[test]
fn mcp_remove_cleans_legacy_gemini_config_without_enabling_setup() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let context = test_context();
        let gemini_path = home.path().join(".gemini/settings.json");
        ensure_parent_dir(&gemini_path).unwrap();
        fs::write(
            &gemini_path,
            serde_json::to_string_pretty(&json!({
                "mcpServers": {
                    "forktty": {
                        "command": "/usr/bin/forktty",
                        "args": ["mcp"],
                        "env": { MCP_MANAGED_ENV: MCP_SERVER_NAME }
                    },
                    "foreign": {
                        "command": "/bin/true"
                    }
                },
                "hooks": {
                    "SessionStart": [{
                        "hooks": [{
                            "type": "command",
                            "command": "custom-gemini-hook"
                        }]
                    }]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        assert_err_contains(
            handle_mcp_setup(&context, strings(&["gemini"])),
            "Unsupported mcp agent: gemini",
        );
        handle_mcp_remove(&context, strings(&["gemini"])).unwrap();

        let config = read_json(&gemini_path);
        assert!(config["mcpServers"].get("forktty").is_none());
        assert_eq!(config["mcpServers"]["foreign"]["command"], "/bin/true");
        assert!(config["hooks"].get("SessionStart").is_some());
    });
}

#[test]
fn skill_setup_writes_default_targets_and_is_idempotent() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();
    let claude_dir_s = claude_dir.path().to_string_lossy().to_string();

    with_env(
        &[
            ("HOME", Some(home_s.as_str())),
            ("CLAUDE_CONFIG_DIR", Some(claude_dir_s.as_str())),
        ],
        || {
            let context = test_context();
            handle_skills_setup(&context, Vec::new()).unwrap();

            let agents_skill = agent_skills_dir();
            let claude_skill = claude_skill_dir();
            for path in [&agents_skill, &claude_skill] {
                let skill = fs::read_to_string(path.join("SKILL.md")).unwrap();
                assert!(skill.contains(AGENT_SKILL_MARKER));
                assert!(skill.contains("context_snapshot"));
                assert!(skill.contains("team_message_dispatch"));
                let metadata = fs::read_to_string(path.join("agents").join("openai.yaml")).unwrap();
                assert!(metadata.contains("value: \"forktty\""));
                assert!(metadata.contains("allow_implicit_invocation: true"));
            }

            let first = fs::read_to_string(agents_skill.join("SKILL.md")).unwrap();
            handle_skills_setup(&context, Vec::new()).unwrap();
            assert_eq!(
                fs::read_to_string(agents_skill.join("SKILL.md")).unwrap(),
                first
            );
            assert_eq!(
                backup_count(
                    agents_skill.parent().unwrap(),
                    "forktty-agent-orchestration.bak-"
                ),
                0
            );
        },
    );
}

#[test]
fn bundled_agent_skill_documents_team_preflight_roles_and_qa_policy() {
    for expected in [
        "## Team Preflight",
        "## Worker Role Templates",
        "## Worktree Policy",
        "## Isolated Integration QA",
        "workflow_upsert",
        "workflow_plan_set",
        "team_task_upsert",
        "agent_health",
        "lifecycle_evidence",
        "include_feed_trace",
        "effective_project_cwd",
        "final_state",
        "source/installed checksums",
        "forktty hooks test codex",
        "FORKTTY_SOCKET_PATH",
        "separate temporary instance",
    ] {
        assert!(
            AGENT_SKILL_MD.contains(expected),
            "agent skill should document {expected}"
        );
    }
}

#[test]
fn skill_setup_pi_alias_targets_interoperable_agents_dir() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();
    let claude_dir_s = claude_dir.path().to_string_lossy().to_string();

    with_env(
        &[
            ("HOME", Some(home_s.as_str())),
            ("CLAUDE_CONFIG_DIR", Some(claude_dir_s.as_str())),
        ],
        || {
            handle_skills_setup(&test_context(), strings(&["pi"])).unwrap();

            assert!(agent_skills_dir().join("SKILL.md").exists());
            assert!(!claude_skill_dir().exists());
        },
    );
}

#[test]
fn skill_setup_rejects_removed_gemini_target() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();
    let claude_dir_s = claude_dir.path().to_string_lossy().to_string();

    with_env(
        &[
            ("HOME", Some(home_s.as_str())),
            ("CLAUDE_CONFIG_DIR", Some(claude_dir_s.as_str())),
        ],
        || {
            let err = handle_skills_setup(&test_context(), strings(&["gemini"])).unwrap_err();
            assert!(
                err.message.contains("Unsupported skills target: gemini"),
                "{err:?}"
            );
            assert!(!agent_skills_dir().exists());
            assert!(!claude_skill_dir().exists());
        },
    );
}

#[test]
fn skill_setup_refuses_unmanaged_existing_skill() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: forktty-agent-orchestration\ndescription: custom\n---\ncustom\n",
        )
        .unwrap();

        assert_err_contains(
            handle_skills_setup(&test_context(), strings(&["agents"])),
            "refusing to overwrite unmanaged skill",
        );
        assert_eq!(
            fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
            "---\nname: forktty-agent-orchestration\ndescription: custom\n---\ncustom\n"
        );
    });
}

#[test]
fn skill_setup_and_doctor_report_managed_skill_drift_checksums() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("{AGENT_SKILL_MARKER}\n# stale\n"),
        )
        .unwrap();
        fs::write(
            skill_dir.join("agents").join("openai.yaml"),
            "stale: true\n",
        )
        .unwrap();

        let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
        let plan = build_skill_setup_plan(spec).unwrap();
        assert_eq!(plan.status, "update_available");
        assert!(plan.changed);
        assert_ne!(plan.installed_checksum, plan.source_checksum);

        let report = build_socket_doctor_report(&test_context());
        assert_eq!(report["skillDirs"]["agents"]["status"], "update_available");
        assert_ne!(
            report["skillDirs"]["agents"]["installedChecksum"],
            report["skillDirs"]["agents"]["sourceChecksum"]
        );
        assert_eq!(
            report["skillDirs"]["agents"]["repairCommand"],
            "forktty skills setup agents"
        );
    });
}

#[test]
fn doctor_reports_symlinked_skill_file_as_unrepairable_invalid() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        let target = home.path().join("external-skill.md");
        fs::write(&target, AGENT_SKILL_MD).unwrap();
        std::os::unix::fs::symlink(&target, skill_dir.join("SKILL.md")).unwrap();
        fs::write(
            skill_dir.join("agents").join("openai.yaml"),
            AGENT_SKILL_OPENAI_YAML,
        )
        .unwrap();

        let report = build_socket_doctor_report(&test_context());

        assert_eq!(report["skillDirs"]["agents"]["status"], "invalid");
        assert!(report["skillDirs"]["agents"]["repairCommand"].is_null());
    });
}

#[test]
fn skill_setup_refuses_symlinked_skill_file_without_verified_marker() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let context = test_context();
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        let target = home.path().join("external-skill.md");
        fs::write(&target, AGENT_SKILL_MD).unwrap();
        std::os::unix::fs::symlink(&target, skill_dir.join("SKILL.md")).unwrap();
        fs::write(
            skill_dir.join("agents").join("openai.yaml"),
            AGENT_SKILL_OPENAI_YAML,
        )
        .unwrap();

        let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
        let error = match build_skill_setup_plan(spec) {
            Ok(_) => panic!("expected symlinked SKILL.md to be refused"),
            Err(error) => error,
        };
        assert!(error
            .message
            .contains("ForkTTY-managed marker could not be verified"));

        let error = handle_skills_setup(&context, strings(&["agents"])).unwrap_err();
        assert!(error
            .message
            .contains("ForkTTY-managed marker could not be verified"));

        let report = build_socket_doctor_report(&context);
        assert_eq!(report["skillDirs"]["agents"]["status"], "invalid");
        assert!(report["skillDirs"]["agents"]["repairCommand"].is_null());
    });
}

#[test]
fn skill_setup_repairs_invalid_metadata_after_verified_marker() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let context = test_context();
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        fs::write(skill_dir.join("SKILL.md"), AGENT_SKILL_MD).unwrap();
        let target = home.path().join("external-openai.yaml");
        fs::write(&target, AGENT_SKILL_OPENAI_YAML).unwrap();
        std::os::unix::fs::symlink(&target, skill_dir.join("agents").join("openai.yaml")).unwrap();

        let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
        let plan = build_skill_setup_plan(spec).unwrap();
        assert_eq!(plan.status, "invalid");
        assert!(plan.changed);

        handle_skills_setup(&context, strings(&["agents"])).unwrap();

        let metadata = fs::symlink_metadata(skill_dir.join("agents").join("openai.yaml")).unwrap();
        assert!(metadata.is_file());
        assert!(!metadata.file_type().is_symlink());
        assert_eq!(
            backup_count(
                skill_dir.parent().unwrap(),
                "forktty-agent-orchestration.bak-"
            ),
            1
        );
        let report = build_socket_doctor_report(&context);
        assert_eq!(report["skillDirs"]["agents"]["status"], "up_to_date");
    });
}

#[test]
fn skill_setup_repairs_symlinked_metadata_dir_after_verified_marker() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let context = test_context();
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), AGENT_SKILL_MD).unwrap();
        let target = home.path().join("external-agents");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("openai.yaml"), AGENT_SKILL_OPENAI_YAML).unwrap();
        std::os::unix::fs::symlink(&target, skill_dir.join("agents")).unwrap();

        let report = build_socket_doctor_report(&context);
        assert_eq!(report["skillDirs"]["agents"]["status"], "invalid");
        assert_eq!(
            report["skillDirs"]["agents"]["repairCommand"],
            "forktty skills setup agents"
        );

        let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
        let plan = build_skill_setup_plan(spec).unwrap();
        assert_eq!(plan.status, "invalid");
        assert!(plan.changed);

        handle_skills_setup(&context, strings(&["agents"])).unwrap();

        let metadata_dir = fs::symlink_metadata(skill_dir.join("agents")).unwrap();
        assert!(metadata_dir.is_dir());
        assert!(!metadata_dir.file_type().is_symlink());
        let metadata = fs::symlink_metadata(skill_dir.join("agents").join("openai.yaml")).unwrap();
        assert!(metadata.is_file());
        assert!(!metadata.file_type().is_symlink());
        let report = build_socket_doctor_report(&context);
        assert_eq!(report["skillDirs"]["agents"]["status"], "up_to_date");
    });
}

#[test]
fn skill_setup_refuses_symlinked_skill_directory_without_verified_marker() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let context = test_context();
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(skill_dir.parent().unwrap()).unwrap();
        let target = home.path().join("external-skill-dir");
        std::os::unix::fs::symlink(&target, &skill_dir).unwrap();

        let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
        let error = match build_skill_setup_plan(spec) {
            Ok(_) => panic!("expected symlinked skill directory to be refused"),
            Err(error) => error,
        };
        assert!(error
            .message
            .contains("ForkTTY-managed marker could not be verified"));

        let error = handle_skills_setup(&context, strings(&["agents"])).unwrap_err();
        assert!(error
            .message
            .contains("ForkTTY-managed marker could not be verified"));

        let skill_dir_meta = fs::symlink_metadata(&skill_dir).unwrap();
        assert!(skill_dir_meta.file_type().is_symlink());
        let report = build_socket_doctor_report(&context);
        assert_eq!(report["skillDirs"]["agents"]["status"], "invalid");
        assert!(report["skillDirs"]["agents"]["repairCommand"].is_null());
    });
}

#[test]
fn skill_setup_summary_reports_repaired_state_after_install() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let skill_dir = agent_skills_dir();
        fs::create_dir_all(skill_dir.join("agents")).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("{AGENT_SKILL_MARKER}\n# stale\n"),
        )
        .unwrap();
        fs::write(
            skill_dir.join("agents").join("openai.yaml"),
            "stale: true\n",
        )
        .unwrap();

        let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
        let plan = build_skill_setup_plan(spec).unwrap();
        assert_eq!(plan.status, "update_available");
        let summary = skill_setup_summary(
            &plan,
            false,
            Some(PathBuf::from("backup")),
            vec![PathBuf::from("old-backup")],
        );

        assert_eq!(summary["changed"], true);
        assert_eq!(summary["status"], "up_to_date");
        assert_eq!(summary["installedChecksum"], summary["sourceChecksum"]);
        assert!(summary["repairCommand"].is_null());
        assert_eq!(summary["removedBackups"][0], "old-backup");
    });
}

#[test]
fn skill_setup_prunes_old_managed_backups_but_keeps_unmanaged_backups() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let context = test_context();
        handle_skills_setup(&context, strings(&["agents"])).unwrap();
        let skill_dir = agent_skills_dir();
        let parent = skill_dir.parent().unwrap().to_path_buf();
        let unmanaged_backup = parent.join("forktty-agent-orchestration.bak-manual");
        fs::create_dir_all(&unmanaged_backup).unwrap();
        fs::write(
            unmanaged_backup.join("SKILL.md"),
            "# manually saved skill\n",
        )
        .unwrap();

        for index in 0..5 {
            fs::write(
                skill_dir.join("SKILL.md"),
                format!("{AGENT_SKILL_MARKER}\n# stale {index}\n"),
            )
            .unwrap();
            handle_skills_setup(&context, strings(&["agents"])).unwrap();
        }

        assert!(unmanaged_backup.exists());
        assert_eq!(backup_count(&parent, "forktty-agent-orchestration.bak-"), 4);
        let managed = fs::read_dir(&parent)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("forktty-agent-orchestration.bak-"))
                    && fs::read_to_string(entry.path().join("SKILL.md"))
                        .is_ok_and(|text| text.contains(AGENT_SKILL_MARKER))
            })
            .count();
        assert_eq!(managed, 3);
    });
}

#[test]
fn skill_remove_moves_managed_skill_to_backup() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        let context = test_context();
        handle_skills_setup(&context, strings(&["agents"])).unwrap();
        let skill_dir = agent_skills_dir();
        assert!(skill_dir.exists());

        handle_skills_remove(&context, strings(&["agents"])).unwrap();

        assert!(!skill_dir.exists());
        assert_eq!(
            backup_count(
                skill_dir.parent().unwrap(),
                "forktty-agent-orchestration.bak-"
            ),
            1
        );
    });
}

#[test]
fn skill_setup_dry_run_and_option_errors_do_not_write() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().to_string_lossy().to_string();

    with_env(&[("HOME", Some(home_s.as_str()))], || {
        handle_skills_setup(&test_context(), strings(&["--dry-run", "agents"])).unwrap();
        assert!(!agent_skills_dir().exists());

        assert_err_contains(
            handle_skills_setup(&test_context(), strings(&["--dry-run=yes", "agents"])),
            "skills setup: --dry-run must be true or false",
        );
        assert_err_contains(
            handle_skills_setup(&test_context(), strings(&["--dryrun", "agents"])),
            "skills setup: unknown option --dryrun",
        );
        assert!(!agent_skills_dir().exists());
    });
}
