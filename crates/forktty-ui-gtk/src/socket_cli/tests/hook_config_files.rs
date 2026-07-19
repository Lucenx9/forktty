use super::*;

#[test]
fn hook_setup_dry_run_and_option_errors_do_not_write_configs() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex");
    let codex_home_s = codex_home.display().to_string();
    let home_s = dir.path().display().to_string();
    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("CLAUDE_CONFIG_DIR", None),
            ("HOME", Some(&home_s)),
        ],
        || {
            let context = test_context();
            handle_hooks_setup(&context, strings(&["--dry-run", "codex"])).unwrap();
            assert!(!codex_home.join("hooks.json").exists());

            assert_err_contains(
                handle_hooks_setup(&context, strings(&["--dry-run=yes", "codex"])),
                "hooks setup: --dry-run must be true or false",
            );
            assert_err_contains(
                handle_hooks_setup(&context, strings(&["--dryrun", "codex"])),
                "hooks setup: unknown option --dryrun",
            );
            assert!(!codex_home.join("hooks.json").exists());
        },
    );
}

#[test]
fn hook_setup_rejects_relative_config_roots() {
    let home = tempfile::tempdir().unwrap();
    let home_s = home.path().display().to_string();

    with_env(
        &[
            ("CODEX_HOME", Some("relative-codex-home")),
            ("CLAUDE_CONFIG_DIR", None),
            ("HOME", Some(&home_s)),
        ],
        || {
            assert_err_contains(
                handle_hooks_setup(&test_context(), strings(&["--dry-run", "codex"])),
                "hook config path must be absolute",
            );
        },
    );
}

#[test]
fn hook_setup_preflights_all_configs_and_prevents_partial_writes() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex");
    let claude_dir = dir.path().join("claude");
    fs::create_dir_all(&codex_home).unwrap();
    fs::create_dir_all(&claude_dir).unwrap();
    let codex_path = codex_home.join("hooks.json");
    let claude_path = claude_dir.join("settings.json");
    fs::write(&codex_path, "{\"customKey\":{\"keepMe\":true}}\n").unwrap();
    fs::write(&claude_path, "{ not json ::: ").unwrap();

    let codex_home_s = codex_home.display().to_string();
    let claude_dir_s = claude_dir.display().to_string();
    let home_s = dir.path().display().to_string();
    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
            ("HOME", Some(&home_s)),
        ],
        || {
            let context = test_context();
            assert_err_contains(
                handle_hooks_setup(&context, strings(&["codex", "claude"])),
                "failed to read claude hook config",
            );
            assert_eq!(
                fs::read_to_string(&codex_path).unwrap(),
                "{\"customKey\":{\"keepMe\":true}}\n"
            );
        },
    );
}

#[test]
fn hook_setup_preserves_unrelated_json() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex");
    fs::create_dir_all(&codex_home).unwrap();
    let codex_path = codex_home.join("hooks.json");
    fs::write(&codex_path, "{\"customKey\":{\"keepMe\":true}}\n").unwrap();

    let codex_home_s = codex_home.display().to_string();
    let home_s = dir.path().display().to_string();
    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("CLAUDE_CONFIG_DIR", None),
            ("HOME", Some(&home_s)),
        ],
        || {
            let context = test_context();
            handle_hooks_setup(&context, strings(&["codex"])).unwrap();
            let parsed = read_json(&codex_path);
            assert_eq!(parsed["customKey"]["keepMe"], true);
            assert!(parsed["hooks"]["SessionStart"].is_array());
            assert_eq!(backup_count(&codex_home, "hooks.json.bak-"), 1);
        },
    );
}

#[test]
fn hook_setup_updates_symlink_targets_without_replacing_link() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex");
    let managed_dir = dir.path().join("managed-codex");
    fs::create_dir_all(&codex_home).unwrap();
    fs::create_dir_all(&managed_dir).unwrap();
    let target_path = managed_dir.join("hooks.json");
    let config_path = codex_home.join("hooks.json");
    fs::write(&target_path, "{\"customKey\":\"managed\"}\n").unwrap();
    std::os::unix::fs::symlink(&target_path, &config_path).unwrap();

    let codex_home_s = codex_home.display().to_string();
    let home_s = dir.path().display().to_string();
    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("CLAUDE_CONFIG_DIR", None),
            ("HOME", Some(&home_s)),
        ],
        || {
            handle_hooks_setup(&test_context(), strings(&["codex"])).unwrap();
            assert!(fs::symlink_metadata(&config_path)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(fs::read_link(&config_path).unwrap(), target_path);
            let parsed = read_json(&target_path);
            assert_eq!(parsed["customKey"], "managed");
            assert!(parsed["hooks"]["SessionStart"].is_array());
            assert_eq!(backup_count(&managed_dir, "hooks.json.bak-"), 1);
            assert_eq!(backup_count(&codex_home, "hooks.json.bak-"), 0);
        },
    );
}

#[test]
fn hook_setup_replaces_broken_symlink_with_regular_file() {
    let dir = tempfile::tempdir().unwrap();
    let codex_home = dir.path().join("codex");
    fs::create_dir_all(&codex_home).unwrap();
    let config_path = codex_home.join("hooks.json");
    std::os::unix::fs::symlink(codex_home.join("missing-target.json"), &config_path).unwrap();

    let codex_home_s = codex_home.display().to_string();
    let home_s = dir.path().display().to_string();
    with_env(
        &[
            ("CODEX_HOME", Some(&codex_home_s)),
            ("CLAUDE_CONFIG_DIR", None),
            ("HOME", Some(&home_s)),
        ],
        || {
            handle_hooks_setup(&test_context(), strings(&["codex"]))
                .expect("setup through broken symlink should succeed");
            let stat = fs::symlink_metadata(&config_path).unwrap();
            assert!(
                stat.is_file(),
                "broken symlink should be replaced by a regular file"
            );
            let parsed = read_json(&config_path);
            assert!(parsed["hooks"]["SessionStart"].is_array());
        },
    );
}

#[test]
fn hook_config_reader_rejects_unsafe_or_invalid_paths() {
    let dir = tempfile::tempdir().unwrap();
    let spec = agent_spec("codex").unwrap();

    let whitespace = dir.path().join("whitespace.json");
    fs::write(&whitespace, "   \n\t  \n").unwrap();
    assert_eq!(read_agent_config(spec, &whitespace).unwrap(), json!({}));

    let array = dir.path().join("array.json");
    fs::write(&array, "[]\n").unwrap();
    assert_err_contains(
        read_agent_config(spec, &array),
        "expected a JSON object at the top level",
    );
    assert_eq!(fs::read_to_string(&array).unwrap(), "[]\n");

    let directory = dir.path().join("directory.json");
    fs::create_dir(&directory).unwrap();
    assert_err_contains(
        read_agent_config(spec, &directory),
        "path exists but is not a regular file",
    );
    assert!(directory.is_dir());

    let oversized = dir.path().join("oversized.json");
    fs::write(
        &oversized,
        vec![b' '; MAX_HOOK_CONFIG_SIZE_BYTES as usize + 1],
    )
    .unwrap();
    assert_err_contains(
        read_agent_config(spec, &oversized),
        "hook config is too large",
    );

    let broken = dir.path().join("broken.json");
    std::os::unix::fs::symlink(dir.path().join("missing.json"), &broken).unwrap();
    assert_eq!(read_agent_config(spec, &broken).unwrap(), json!({}));
    assert!(fs::symlink_metadata(&broken)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn atomic_write_replaces_target_and_cleans_temp_on_failure() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("atomic.json");
    atomic_write_file(&target, b"first\n").unwrap();
    atomic_write_file(&target, b"second\n").unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "second\n");
    assert_eq!(backup_count(dir.path(), ".atomic.json.tmp-"), 0);

    let target_in_missing_dir = dir.path().join("missing").join("atomic.json");
    assert!(atomic_write_file(&target_in_missing_dir, b"content\n").is_err());
    assert!(!target_in_missing_dir.exists());
    assert_eq!(backup_count(dir.path(), ".atomic.json.tmp-"), 0);
}

fn runtime_vars(appimage: &str, appdir: &str) -> [(Option<OsString>, Option<OsString>); 2] {
    [
        (Some(OsString::from(appimage)), Some(OsString::from(appdir))),
        (None, None),
    ]
}

fn forktty_vars(appimage: &str, appdir: &str) -> [(Option<OsString>, Option<OsString>); 2] {
    [
        (None, None),
        (Some(OsString::from(appimage)), Some(OsString::from(appdir))),
    ]
}

#[test]
fn stable_hook_launcher_uses_appimage_only_for_appdir_binary() {
    assert_eq!(
        stable_hook_launcher_path_from_env(
            Some(Path::new("/tmp/.mount_forktty/usr/bin/forktty")),
            runtime_vars("/home/me/forktty.AppImage", "/tmp/.mount_forktty"),
        ),
        Some(PathBuf::from("/home/me/forktty.AppImage"))
    );
    assert_eq!(
        stable_hook_launcher_path_from_env(
            Some(Path::new("/usr/bin/forktty")),
            runtime_vars("/home/me/forktty.AppImage", "/tmp/.mount_forktty"),
        ),
        Some(PathBuf::from("/usr/bin/forktty"))
    );
}

// Shells inside the AppImage app see FORKTTY_APPIMAGE/FORKTTY_APPIMAGE_DIR
// (exported by AppRun) instead of the runtime's APPIMAGE/APPDIR, which are
// stripped from child environments. Hooks set up from such a shell used to
// embed the volatile /tmp/.mount_* binary path, which broke on remount.
#[test]
fn stable_hook_launcher_uses_forktty_appimage_vars_from_child_shells() {
    assert_eq!(
        stable_hook_launcher_path_from_env(
            Some(Path::new("/tmp/.mount_forktty/usr/bin/forktty")),
            forktty_vars("/home/me/forktty.AppImage", "/tmp/.mount_forktty"),
        ),
        Some(PathBuf::from("/home/me/forktty.AppImage"))
    );
    // A dev binary invoked explicitly from inside an AppImage shell must
    // keep its own path: it is not the mounted binary.
    assert_eq!(
        stable_hook_launcher_path_from_env(
            Some(Path::new("/home/me/forktty/target/release/forktty")),
            forktty_vars("/home/me/forktty.AppImage", "/tmp/.mount_forktty"),
        ),
        Some(PathBuf::from("/home/me/forktty/target/release/forktty"))
    );
}

#[test]
fn parse_launcher_extracts_path_from_managed_command() {
    let spec = agent_spec("claude").unwrap();
    let command = build_hook_shell_command(
        Path::new("/home/me/ForkTTY/forktty.AppImage"),
        spec,
        "session-start",
    );
    assert_eq!(
        parse_launcher_from_managed_command(&command, spec).as_deref(),
        Some("/home/me/ForkTTY/forktty.AppImage")
    );
}

#[test]
fn appimage_hook_commands_use_extract_and_run_env() {
    let spec = agent_spec("codex").unwrap();
    let launcher = Path::new("/home/me/AppImages/forktty.appimage");
    let command = build_hook_shell_command(launcher, spec, "pre-tool");

    assert!(command.contains(
        "&& APPIMAGE_EXTRACT_AND_RUN=1 '/home/me/AppImages/forktty.appimage' hooks codex pre-tool"
    ));
    assert_eq!(
        parse_launcher_from_managed_command(&command, spec).as_deref(),
        Some("/home/me/AppImages/forktty.appimage")
    );

    let native = build_hook_shell_command(Path::new("/usr/bin/forktty"), spec, "pre-tool");
    assert!(!native.contains("APPIMAGE_EXTRACT_AND_RUN"));
}

#[test]
fn renamed_appimage_hook_commands_use_runtime_env_provenance() {
    let spec = agent_spec("codex").unwrap();
    let launcher = "/home/me/AppImages/forktty-renamed";

    with_env(&[("APPIMAGE", Some(launcher))], || {
        let command = build_hook_shell_command(Path::new(launcher), spec, "pre-tool");
        assert!(command.contains(
            "&& APPIMAGE_EXTRACT_AND_RUN=1 '/home/me/AppImages/forktty-renamed' hooks codex pre-tool"
        ));

        let native = build_hook_shell_command(Path::new("/usr/bin/forktty"), spec, "pre-tool");
        assert!(!native.contains("APPIMAGE_EXTRACT_AND_RUN"));
    });
}

#[test]
fn parse_launcher_handles_apostrophe_in_path() {
    let spec = agent_spec("codex").unwrap();
    let command =
        build_hook_shell_command(Path::new("/home/o'connor/forktty"), spec, "session-start");
    assert_eq!(
        parse_launcher_from_managed_command(&command, spec).as_deref(),
        Some("/home/o'connor/forktty")
    );
}

#[test]
fn parse_launcher_rejects_unrelated_commands() {
    let spec = agent_spec("codex").unwrap();
    assert_eq!(parse_launcher_from_managed_command("echo hi", spec), None);
    assert_eq!(
        parse_launcher_from_managed_command(
            "[ x ] && '/usr/bin/forktty' hooks claude session-start || true",
            spec,
        ),
        None,
        "wrong agent key must not match"
    );
}

#[test]
fn extract_managed_launcher_returns_first_forktty_entry() {
    let spec = agent_spec("claude").unwrap();
    let (_, config) = merge_hook_config(&json!({}), spec, Path::new("/usr/bin/forktty")).unwrap();
    assert_eq!(
        extract_managed_launcher_from_config(spec, &config).as_deref(),
        Some("/usr/bin/forktty")
    );
}

#[test]
fn extract_managed_launcher_accepts_legacy_forktty_entries() {
    let spec = agent_spec("claude").unwrap();
    let command = build_hook_shell_command(Path::new("/usr/bin/forktty"), spec, "session-start");
    let config = json!({
        "hooks": {
            "SessionStart": [{
                "matcher": "*",
                "hooks": [{
                    "type": "command",
                    "command": command,
                    "timeout": 30,
                    "statusMessage": "ForkTTY Claude hooks"
                }]
            }]
        }
    });

    assert_eq!(
        extract_managed_launcher_from_config(spec, &config).as_deref(),
        Some("/usr/bin/forktty")
    );
}

#[test]
fn extract_managed_launcher_skips_unmanaged_entries() {
    let spec = agent_spec("codex").unwrap();
    let config = json!({
        "hooks": {
            "SessionStart": [{
                "hooks": [{
                    "type": "command",
                    "command": "echo unrelated",
                }]
            }]
        }
    });
    assert_eq!(extract_managed_launcher_from_config(spec, &config), None);
}

#[test]
fn describe_launcher_check_flags_stale_path() {
    let dir = tempfile::tempdir().unwrap();
    let spec = agent_spec("claude").unwrap();
    let config_path = dir.path().join("settings.json");
    let (_, config) = merge_hook_config(&json!({}), spec, Path::new("/old/forktty")).unwrap();
    fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();
    let check = describe_launcher_check(spec, &config_path, Some(Path::new("/new/forktty")));
    assert_eq!(check["status"], Value::String("stale".to_string()));
    assert_eq!(
        check["installedLauncher"],
        Value::String("/old/forktty".to_string())
    );
    assert_eq!(
        check["currentLauncher"],
        Value::String("/new/forktty".to_string())
    );
}

#[test]
#[cfg(unix)]
fn describe_launcher_check_treats_working_recorded_launcher_as_ok() {
    use std::os::unix::fs::PermissionsExt;

    // The recorded launcher exists and is executable, but the process now
    // runs from a different path (e.g. an AppImage version-suffixed copy).
    // The hooks still work, so this must not be reported as stale.
    let dir = tempfile::tempdir().unwrap();
    let installed_launcher = dir.path().join("forktty.appimage");
    fs::write(&installed_launcher, b"#!/bin/sh\n").unwrap();
    fs::set_permissions(&installed_launcher, fs::Permissions::from_mode(0o755)).unwrap();

    let spec = agent_spec("claude").unwrap();
    let config_path = dir.path().join("settings.json");
    let (_, config) = merge_hook_config(&json!({}), spec, &installed_launcher).unwrap();
    fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();

    let current = dir.path().join("forktty.appimage_0_2_0-alpha_7.appimage");
    let check = describe_launcher_check(spec, &config_path, Some(&current));
    assert_eq!(check["status"], Value::String("ok".to_string()));
    assert_eq!(
        check["installedLauncher"],
        Value::String(installed_launcher.display().to_string())
    );
}

#[test]
fn describe_launcher_check_marks_matching_path_as_ok() {
    let dir = tempfile::tempdir().unwrap();
    let spec = agent_spec("codex").unwrap();
    let config_path = dir.path().join("hooks.json");
    let (_, config) = merge_hook_config(&json!({}), spec, Path::new("/usr/bin/forktty")).unwrap();
    fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();
    let check = describe_launcher_check(spec, &config_path, Some(Path::new("/usr/bin/forktty")));
    assert_eq!(check["status"], Value::String("ok".to_string()));
}

#[test]
fn describe_launcher_check_marks_missing_config_as_not_installed() {
    let dir = tempfile::tempdir().unwrap();
    let spec = agent_spec("opencode").unwrap();
    let config_path = dir.path().join("forktty.generated.js");
    let check = describe_launcher_check(spec, &config_path, Some(Path::new("/usr/bin/forktty")));
    assert_eq!(check["status"], Value::String("not_installed".to_string()));
}
