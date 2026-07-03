use super::*;

#[cfg(unix)]
fn mkfifo(path: &Path) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes()).unwrap();
    let rc = unsafe { libc::mkfifo(path.as_ptr(), 0o600) };
    assert_eq!(rc, 0, "mkfifo failed: {}", std::io::Error::last_os_error());
}

#[test]
fn load_config_from_path_with_recovery_recovers_bad_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "invalid_toml = { [ ] }").unwrap();

    let (config, recovery) = load_config_from_path_with_recovery(&path).unwrap();

    // Should return default config
    assert_eq!(config.appearance.font_size, 14);

    // Should return a ConfigRecovery object
    let recovery = recovery.expect("Expected ConfigRecovery when loading bad config");
    assert!(recovery.reason.contains("TOML parse error"));

    let quarantined_path = recovery
        .quarantined_path
        .expect("Expected quarantined path");
    assert!(quarantined_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains(".bad-"));

    // Original config should be moved
    assert!(!path.exists());
    assert!(quarantined_path.exists());
    assert_eq!(
        fs::read_to_string(&quarantined_path).unwrap(),
        "invalid_toml = { [ ] }"
    );
}

#[test]
fn load_config_from_path_with_recovery_loads_good_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [appearance]
            font_size = 20
            "#,
    )
    .unwrap();

    let (config, recovery) = load_config_from_path_with_recovery(&path).unwrap();

    assert_eq!(config.appearance.font_size, 20);
    assert!(recovery.is_none());
    assert!(path.exists());
}

#[test]
fn missing_config_loads_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let config = load_config_from_path(&dir.path().join("missing.toml")).unwrap();
    assert_eq!(config.appearance.font_size, 14);
    assert_eq!(config.appearance.scrollback_lines, 20_000);
    assert_eq!(config.appearance.persistent_scrollback_lines, 0);
    assert!(config.appearance.terminal_audible_bell);
    assert_eq!(config.appearance.terminal_theme, TERMINAL_THEME_SYSTEM);
    assert!(!config.general.enable_pr_lookup);
    // PTY persistence is opt-in: defaults must not change how shells spawn.
    assert!(!config.general.persist_terminal_processes);
}

#[test]
fn telemetry_anonymous_ping_defaults_to_enabled() {
    assert!(AppConfig::default().telemetry.anonymous_ping);
}

#[test]
fn telemetry_anonymous_ping_can_be_disabled_from_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [telemetry]
            anonymous_ping = false
            "#,
    )
    .unwrap();

    let config = load_config_from_path(&path).unwrap();

    assert!(!config.telemetry.anonymous_ping);
}

#[test]
fn embedded_ghostty_defaults_to_on() {
    assert!(AppConfig::default().appearance.embedded_ghostty);
    let dir = tempfile::tempdir().unwrap();
    let config = load_config_from_path(&dir.path().join("missing.toml")).unwrap();
    assert!(config.appearance.embedded_ghostty);
}

#[test]
fn embedded_ghostty_legacy_key_can_be_loaded_from_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [appearance]
            embedded_ghostty = false
            "#,
    )
    .unwrap();

    let config = load_config_from_path(&path).unwrap();

    assert!(!config.appearance.embedded_ghostty);
}

#[test]
fn embedded_ghostty_legacy_opt_out_is_not_preserved_on_save() {
    // The settings dialog saves via update_config_*, which loads then
    // mutates then writes the whole config. The old temporary opt-out key
    // is accepted on load, but saves should drop it and return to the
    // current always-embedded runtime behavior.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [appearance]
            embedded_ghostty = false
            "#,
    )
    .unwrap();

    update_config_at_path_if_changed(&path, |config| {
        config.general.enable_pr_lookup = true;
    })
    .unwrap();

    let config = load_config_from_path(&path).unwrap();
    assert!(config.appearance.embedded_ghostty);
    assert!(config.general.enable_pr_lookup);
    let saved = fs::read_to_string(&path).unwrap();
    assert!(!saved.contains("embedded_ghostty"));
}

#[test]
fn embedded_ghostty_legacy_key_is_dropped_on_save() {
    // Embedded Ghostty is the only runtime renderer now. Older config files
    // with the temporary opt-out key should load without failing, but new
    // saves should remove the key instead of preserving a non-functional
    // switch.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [appearance]
            embedded_ghostty = false
            "#,
    )
    .unwrap();

    update_config_at_path_if_changed(&path, |config| {
        config.general.enable_pr_lookup = true;
    })
    .unwrap();

    let saved = fs::read_to_string(&path).unwrap();
    assert!(!saved.contains("embedded_ghostty"));
}

#[test]
fn team_config_defaults_to_auto_provider_policy() {
    let config = AppConfig::default();

    assert_eq!(config.team.default_agent, "auto");
    assert_eq!(
        config.team.provider_order,
        ["codex", "claude", "pi", "opencode", "antigravity", "grok"]
    );
    assert!(config.team.auto_fallback);
    assert!(config.team.disabled_agents.is_empty());
    assert!(config.team.provider_commands.is_empty());
}

#[test]
fn team_config_normalizes_provider_aliases_and_drops_invalid_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
            &path,
            r#"
            [general]
            shell = "/bin/sh"

            [team]
            default_agent = "claude-code"
            provider_order = ["Pi", "unknown", "codex", "pi", "agy", "grok-build"]
            auto_fallback = false
            disabled_agents = ["open-code", "bad", "codex", "codex"]
            provider_commands = { "claude-code" = " /opt/claude/bin/claude ", agy = "agy-dev", "grok-build" = " /opt/grok/bin/grok ", unknown = "ignored" }
            "#,
        )
        .unwrap();

    let config = load_config_from_path(&path).unwrap();

    assert_eq!(config.team.default_agent, "claude");
    assert_eq!(
        config.team.provider_order,
        ["pi", "codex", "antigravity", "grok"]
    );
    assert!(!config.team.auto_fallback);
    assert_eq!(config.team.disabled_agents, ["opencode", "codex"]);
    assert_eq!(
        config
            .team
            .provider_commands
            .get("claude")
            .map(String::as_str),
        Some("/opt/claude/bin/claude")
    );
    assert_eq!(
        config
            .team
            .provider_commands
            .get("antigravity")
            .map(String::as_str),
        Some("agy-dev")
    );
    assert_eq!(
        config
            .team
            .provider_commands
            .get("grok")
            .map(String::as_str),
        Some("/opt/grok/bin/grok")
    );
    assert!(!config.team.provider_commands.contains_key("unknown"));
}

#[test]
fn team_provider_command_values_reject_args_and_relative_paths() {
    for valid in ["codex", "/opt/codex/bin/codex", "/opt/My Tools/codex"] {
        assert!(
            validate_team_provider_command_value(valid).is_ok(),
            "{valid} should be accepted"
        );
    }
    for invalid in [
        "codex --model fast",
        "./codex",
        "bin/codex",
        "-codex",
        "co\ndex",
    ] {
        assert!(
            validate_team_provider_command_value(invalid).is_err(),
            "{invalid:?} should be rejected"
        );
    }
}

#[test]
fn default_shell_uses_executable_shell_env() {
    assert_eq!(
        default_shell_from_env(Some("/bin/sh".to_string())),
        "/bin/sh"
    );
}

#[test]
fn validate_config_rejects_empty_shell() {
    let mut config = AppConfig::default();
    config.general.shell = "   ".to_string();
    let err = validate_config(&config).unwrap_err();
    assert!(err.to_string().contains("general.shell must not be empty"));
}

#[test]
fn validate_config_rejects_non_executable_shell() {
    let mut config = AppConfig::default();
    config.general.shell = "not_an_absolute_path".to_string();
    let err = validate_config(&config).unwrap_err();
    assert!(err
        .to_string()
        .contains("general.shell must be an absolute path"));
}

fn dummy_executable_path() -> String {
    std::env::current_exe()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn validate_config_rejects_invalid_worktree_layout() {
    let mut config = AppConfig::default();
    config.general.shell = dummy_executable_path();
    config.general.worktree_layout = "invalid_layout".to_string();
    let err = validate_config(&config).unwrap_err();
    assert!(err
        .to_string()
        .contains("general.worktree_layout must be one of: nested, sibling, outer-nested"));
}

#[test]
fn validate_config_rejects_invalid_sidebar_position() {
    let mut config = AppConfig::default();
    config.general.shell = dummy_executable_path();
    config.appearance.sidebar_position = "top".to_string();
    let err = validate_config(&config).unwrap_err();
    assert!(err
        .to_string()
        .contains("appearance.sidebar_position must be 'left' or 'right'"));
}

#[test]
fn validate_config_accepts_legacy_font_size() {
    let mut config = AppConfig::default();
    config.general.shell = dummy_executable_path();
    config.appearance.font_size = 7;
    validate_config(&config).unwrap();

    config.appearance.font_size = 65;
    validate_config(&config).unwrap();
}

#[test]
fn validate_config_rejects_invalid_terminal_renderer() {
    let mut config = AppConfig::default();
    config.general.shell = dummy_executable_path();
    config.appearance.terminal_renderer = "magic".to_string();
    let err = validate_config(&config).unwrap_err();
    assert!(err.to_string().contains(
        "appearance.terminal_renderer must be one of: auto, dom, canvas, webgl, ghostty, vte"
    ));
}

#[test]
fn validate_config_accepts_legacy_terminal_theme() {
    let mut config = AppConfig::default();
    config.general.shell = dummy_executable_path();
    config.appearance.terminal_theme = "nonexistent_theme".to_string();

    validate_config(&config).unwrap();
}

#[test]
fn validate_config_rejects_invalid_window_mode() {
    let mut config = AppConfig::default();
    config.general.shell = dummy_executable_path();
    config.appearance.window_mode = "fullscreen".to_string();
    let err = validate_config(&config).unwrap_err();
    assert!(err
        .to_string()
        .contains("appearance.window_mode must be one of: normal, quake"));
}

#[test]
fn default_shell_ignores_relative_or_missing_shell_env() {
    let relative = default_shell_from_env(Some("zsh".to_string()));
    assert!(Path::new(&relative).is_absolute());
    assert!(is_executable_file(Path::new(&relative)));

    let missing = default_shell_from_env(Some("/definitely/missing/forktty-shell".to_string()));
    assert!(Path::new(&missing).is_absolute());
    assert!(is_executable_file(Path::new(&missing)));
}

#[test]
fn accepts_legacy_vte_renderer_for_config_compatibility() {
    let mut config = AppConfig::default();
    config.general.shell = "/bin/sh".to_string();
    config.appearance.terminal_renderer = "vte".to_string();
    validate_config(&config).unwrap();
}

#[test]
fn legacy_vte_renderer_normalizes_to_auto() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [general]
            shell = "/bin/sh"

            [appearance]
            terminal_renderer = "vte"
            "#,
    )
    .unwrap();

    let config = load_config_from_path(&path).unwrap();

    assert_eq!(config.appearance.terminal_renderer, "auto");
}

#[test]
fn loaded_config_rejects_invalid_saved_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    // A shell `-c` trampoline is structurally invalid (not merely
    // environment-dependent), so loading must still hard-error.
    fs::write(
        &path,
        r#"
            [general]
            shell = "/bin/sh"
            notification_command = "/bin/sh -c notify-send"
            "#,
    )
    .unwrap();

    assert!(matches!(
        load_config_from_path(&path),
        Err(ConfigError::Invalid(_))
    ));
}

#[test]
fn loaded_config_accepts_file_at_exactly_max_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let base = "[general]\nshell = \"/bin/sh\"\n";
    let pad = MAX_CONFIG_SIZE_BYTES as usize - base.len();
    // Pad to exactly the cap with a trailing comment. The read uses
    // `take(MAX + 1)` and rejects only when content exceeds MAX, so a file
    // sitting precisely at the boundary must still load.
    let content = format!("{base}#{}", "a".repeat(pad - 1));
    assert_eq!(content.len() as u64, MAX_CONFIG_SIZE_BYTES);
    fs::write(&path, &content).unwrap();

    let config = load_config_from_path(&path).unwrap();

    assert_eq!(config.general.shell, "/bin/sh");
}

#[test]
fn loaded_config_normalizes_choice_values_from_manual_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [general]
            shell = "/bin/sh"
            worktree_layout = " SIBLING "
            enable_pr_lookup = true

            [appearance]
            sidebar_position = " Right "
            terminal_renderer = " VTE "
            terminal_theme = " Tokyo-Night "
            window_mode = " Quake "
            "#,
    )
    .unwrap();

    let config = load_config_from_path(&path).unwrap();

    assert_eq!(config.general.worktree_layout, "sibling");
    assert!(config.general.enable_pr_lookup);
    assert_eq!(config.appearance.sidebar_position, "right");
    assert_eq!(config.appearance.terminal_renderer, "auto");
    assert_eq!(config.appearance.terminal_theme, TERMINAL_THEME_TOKYO_NIGHT);
    assert_eq!(config.appearance.window_mode, "quake");
}

#[test]
fn loaded_config_trims_shell_path_from_manual_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [general]
            shell = " /bin/sh "
            "#,
    )
    .unwrap();

    let config = load_config_from_path(&path).unwrap();

    assert_eq!(config.general.shell, "/bin/sh");
}

#[test]
fn loaded_config_trims_notification_command_from_manual_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [general]
            shell = "/bin/sh"
            notification_command = " /bin/true "
            "#,
    )
    .unwrap();

    let config = load_config_from_path(&path).unwrap();

    assert_eq!(config.general.notification_command, "/bin/true");
}

#[test]
fn loaded_config_normalizes_comment_only_notification_command_without_quarantine() {
    // A notification_command that tokenizes to zero words (e.g. an inline
    // shell comment) is benign — equivalent to "no command" — so it must be
    // normalized away on load like a missing/non-executable program, not
    // quarantine the entire config the way a structural problem does.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r##"
            [general]
            shell = "/bin/sh"
            notification_command = "# disabled for now"

            [appearance]
            font_size = 20
            "##,
    )
    .unwrap();

    let (config, recovery) = load_config_from_path_with_recovery(&path).unwrap();

    assert!(recovery.is_none(), "config should not be quarantined");
    assert_eq!(config.general.notification_command, "");
    assert_eq!(config.appearance.font_size, 20);
}

#[test]
fn loaded_config_accepts_legacy_theme_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [general]
            shell = "/bin/sh"
            theme_source = "light"
            "#,
    )
    .unwrap();

    let config = load_config_from_path(&path).unwrap();

    assert_eq!(config.general.shell, "/bin/sh");
}

#[test]
fn saved_config_omits_legacy_theme_source_from_loaded_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [general]
            theme_source = "dark"
            shell = "/bin/sh"
            "#,
    )
    .unwrap();

    let config = load_config_from_path(&path).unwrap();
    save_config_to_path(&path, &config).unwrap();
    let saved = fs::read_to_string(&path).unwrap();

    assert!(!saved.contains("theme_source"));
}

#[test]
fn saved_config_omits_legacy_terminal_appearance_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut config = AppConfig::default();
    config.general.shell = "/bin/sh".to_string();
    config.appearance.font_family = "Hack".to_string();
    config.appearance.font_size = 99;
    config.appearance.scrollback_lines = 42_000;
    config.appearance.terminal_audible_bell = false;
    config.appearance.terminal_renderer = "ghostty".to_string();
    config.appearance.terminal_theme = "solarized".to_string();

    save_config_to_path(&path, &config).unwrap();
    let saved = fs::read_to_string(&path).unwrap();

    assert!(!saved.contains("font_family"));
    assert!(!saved.contains("font_size"));
    assert!(!saved
        .lines()
        .any(|line| line.trim_start().starts_with("scrollback_lines =")));
    assert!(!saved.contains("terminal_audible_bell"));
    assert!(!saved.contains("terminal_renderer"));
    assert!(!saved.contains("terminal_theme"));
}

#[test]
fn loaded_config_normalizes_terminal_notification_filters() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [general]
            shell = "/bin/sh"

            [notifications]
            blocked_terminal_apps = [" make ", "", " cargo "]
            blocked_terminal_types = [" build.error ", "ci"]
            "#,
    )
    .unwrap();

    let config = load_config_from_path(&path).unwrap();

    assert_eq!(
        config.notifications.blocked_terminal_apps,
        ["make", "cargo"]
    );
    assert_eq!(
        config.notifications.blocked_terminal_types,
        ["build.error", "ci"]
    );
}

#[test]
fn validate_config_rejects_overlong_terminal_notification_filter() {
    let mut config = AppConfig::default();
    config.general.shell = dummy_executable_path();
    config.notifications.blocked_terminal_apps =
        vec!["x".repeat(MAX_NOTIFICATION_FILTER_VALUE_CHARS + 1)];

    let err = validate_config(&normalize_loaded_config(config)).unwrap_err();
    assert!(err.to_string().contains("blocked_terminal_apps entries"));
}

fn assert_recovery_and_get_quarantined_path(
    config: AppConfig,
    recovery: Option<ConfigRecovery>,
) -> (ConfigRecovery, PathBuf) {
    assert_eq!(config, AppConfig::default());
    let recovery = recovery.expect("expected recovery details");
    let quarantined_path = recovery
        .quarantined_path
        .clone()
        .expect("expected quarantined path");
    (recovery, quarantined_path)
}

#[test]
fn recovery_quarantines_corrupt_config_and_returns_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "{ not toml").unwrap();

    let (config, recovery) = load_config_from_path_with_recovery(&path).unwrap();

    assert!(!path.exists(), "bad config should be renamed aside");
    let (recovery, quarantined_path) = assert_recovery_and_get_quarantined_path(config, recovery);
    assert!(recovery.reason.contains("TOML"));
    assert!(quarantined_path.exists());
    assert!(quarantined_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains(".bad-"));
}

#[cfg(unix)]
#[test]
fn recovery_quarantines_broken_config_symlink_and_returns_defaults() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    symlink(dir.path().join("missing-config.toml"), &path).unwrap();

    let (config, recovery) = load_config_from_path_with_recovery(&path).unwrap();

    assert!(
        fs::symlink_metadata(&path).is_err(),
        "broken config symlink should be renamed aside"
    );
    let (_, quarantined_path) = assert_recovery_and_get_quarantined_path(config, recovery);
    assert!(fs::symlink_metadata(&quarantined_path)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(unix)]
#[test]
fn recovery_quarantines_symlink_to_fifo_config_without_blocking() {
    use std::os::unix::fs::symlink;
    use std::sync::mpsc;
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let fifo = dir.path().join("config-fifo");
    mkfifo(&fifo);
    symlink(&fifo, &path).unwrap();

    let path_for_thread = path.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = load_config_from_path_with_recovery(&path_for_thread);
        let _ = tx.send(result);
    });
    let (config, recovery) = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("loading a FIFO-backed config path must not block")
        .unwrap();

    assert_eq!(config.appearance.font_size, default_font_size());
    let recovery = recovery.expect("expected recovery details");
    assert!(recovery.reason.contains("not a regular file"));
    assert!(
        fs::symlink_metadata(&path).is_err(),
        "config symlink should be renamed aside"
    );
    assert!(
        fifo.exists(),
        "quarantining the symlink must not rename its target"
    );
    let quarantined_path = recovery
        .quarantined_path
        .expect("expected quarantined path");
    assert!(fs::symlink_metadata(&quarantined_path)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn recovery_quarantines_config_directory_and_returns_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("note.txt"), "not a config").unwrap();

    let (config, recovery) = load_config_from_path_with_recovery(&path).unwrap();

    assert!(
        !path.exists(),
        "bad config directory should be renamed aside"
    );
    let (_, quarantined_path) = assert_recovery_and_get_quarantined_path(config, recovery);
    assert!(quarantined_path.is_dir());
    assert_eq!(
        fs::read_to_string(quarantined_path.join("note.txt")).unwrap(),
        "not a config"
    );
}

#[cfg(unix)]
#[test]
fn recovery_propagates_unreadable_config_without_quarantine() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "[general]\nshell = \"/bin/sh\"\n").unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o000);
    fs::set_permissions(&path, permissions).unwrap();
    if fs::File::open(&path).is_ok() {
        return;
    }

    let result = load_config_from_path_with_recovery(&path);

    let err = result.expect_err("unreadable config must surface the I/O error");
    assert!(matches!(err, ConfigError::Io(_)));
    assert!(path.exists(), "I/O errors must not quarantine valid config");
}

#[test]
fn recovery_does_not_overwrite_existing_quarantine_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let first_candidate = path.with_extension("toml.bad-20260521010203");
    fs::write(&path, "new bad config").unwrap();
    fs::write(&first_candidate, "previous bad config").unwrap();

    let quarantine_path = quarantine_bad_config_with_timestamp(&path, "20260521010203")
        .unwrap()
        .unwrap();

    assert_ne!(quarantine_path, first_candidate);
    assert!(!path.exists());
    assert_eq!(
        fs::read_to_string(&first_candidate).unwrap(),
        "previous bad config"
    );
    assert_eq!(
        fs::read_to_string(&quarantine_path).unwrap(),
        "new bad config"
    );
}

#[test]
fn available_bad_config_path_reserves_the_returned_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let reserved = available_bad_config_path(&path, "20260521010203", BackupReservationKind::File);

    assert!(reserved.exists(), "candidate must be atomically reserved");
}

#[test]
fn recovery_warning_names_quarantined_config() {
    let recovery = ConfigRecovery {
        reason: "TOML parse error".to_string(),
        quarantined_path: Some(PathBuf::from("/tmp/config.toml.bad-1")),
    };

    let warning = format_config_recovery_warning(&recovery);

    assert!(warning.contains("/tmp/config.toml.bad-1"));
    assert!(warning.contains("defaults are in use"));
    assert!(warning.contains("TOML parse error"));
}

#[test]
fn notification_command_rejects_shell_trampoline() {
    let mut config = AppConfig::default();
    config.general.shell = "/bin/sh".to_string();
    config.general.notification_command = "/bin/sh -c notify-send".to_string();

    let error = validate_config(&config).unwrap_err();

    assert!(error.to_string().contains("must not invoke a shell"));
}

#[cfg(unix)]
#[test]
fn notification_command_accepts_ssh_cipher_option() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join("ssh");
    fs::write(&ssh, "").unwrap();
    let mut permissions = fs::metadata(&ssh).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&ssh, permissions).unwrap();

    let mut config = AppConfig::default();
    config.general.shell = "/bin/sh".to_string();
    config.general.notification_command =
        format!("{} -c aes128-ctr host.example.com", ssh.display());

    validate_config(&config).unwrap();
}

#[cfg(unix)]
#[test]
fn notification_command_rejects_shell_trampoline_after_option_value() {
    let dir = tempfile::tempdir().unwrap();
    let bash = dir.path().join("bash");
    fs::write(&bash, "").unwrap();
    let mut permissions = fs::metadata(&bash).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&bash, permissions).unwrap();

    let mut config = AppConfig::default();
    config.general.shell = "/bin/sh".to_string();
    config.general.notification_command = format!("{} -o vi -c notify-send", bash.display());

    let error = validate_config(&config).unwrap_err();

    assert!(error.to_string().contains("must not invoke a shell"));
}

#[test]
fn sidebar_visible_defaults_to_true_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let config = load_config_from_path(&dir.path().join("missing.toml")).unwrap();
    assert!(config.appearance.sidebar_visible);
}

#[test]
fn pr_lookup_defaults_to_disabled_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [general]
            shell = "/bin/sh"
            "#,
    )
    .unwrap();

    let config = load_config_from_path(&path).unwrap();

    assert!(!config.general.enable_pr_lookup);
}

#[test]
fn loaded_config_clamps_out_of_range_numeric_fields_instead_of_quarantining() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [general]
            shell = "/bin/sh"
            worktree_layout = "sibling"

            [appearance]
            font_size = 999
            scrollback_lines = 9000000
            "#,
    )
    .unwrap();

    let config = load_config_from_path(&path).unwrap();

    // Out-of-range numerics are clamped on load rather than failing
    // validation and quarantining the whole config.
    assert_eq!(config.appearance.font_size, 64);
    assert_eq!(config.appearance.scrollback_lines, 500_000);
    // Unrelated fields survive instead of being reset to defaults.
    assert_eq!(config.general.worktree_layout, "sibling");
}

#[test]
fn loaded_config_falls_back_to_default_shell_instead_of_quarantining() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [general]
            shell = "/definitely/missing/forktty-shell"
            worktree_layout = "sibling"
            "#,
    )
    .unwrap();

    let (config, recovery) = load_config_from_path_with_recovery(&path).unwrap();

    // A missing shell is environment-dependent: normalize on load instead
    // of quarantining the whole config and reverting it to defaults.
    assert!(recovery.is_none());
    assert!(path.exists(), "config file must not be renamed/quarantined");
    assert_eq!(config.general.shell, default_shell());
    assert!(is_executable_file(Path::new(&config.general.shell)));
    // Unrelated fields survive instead of being reset to defaults.
    assert_eq!(config.general.worktree_layout, "sibling");
}

#[test]
fn loaded_config_clears_missing_notification_command_instead_of_quarantining() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"
            [general]
            shell = "/bin/sh"
            notification_command = "/definitely/missing/forktty-notify --flag"
            "#,
    )
    .unwrap();

    let (config, recovery) = load_config_from_path_with_recovery(&path).unwrap();

    assert!(recovery.is_none());
    assert!(path.exists(), "config file must not be renamed/quarantined");
    assert_eq!(config.general.notification_command, "");
}

#[test]
fn loaded_config_clamps_small_font_size_up_to_minimum() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "[general]\nshell = \"/bin/sh\"\n\n[appearance]\nfont_size = 3\n",
    )
    .unwrap();

    let config = load_config_from_path(&path).unwrap();

    assert_eq!(config.appearance.font_size, 8);
}

#[test]
fn scrollback_lines_are_bounded() {
    let mut config = AppConfig::default();
    config.general.shell = "/bin/sh".to_string();
    config.appearance.scrollback_lines = 500_001;

    let error = validate_config(&config).unwrap_err();

    assert!(error.to_string().contains("scrollback_lines"));
}

#[test]
fn persistent_scrollback_lines_are_bounded() {
    let mut config = AppConfig::default();
    config.general.shell = "/bin/sh".to_string();
    config.appearance.persistent_scrollback_lines = 1_001;

    let error = validate_config(&config).unwrap_err();

    assert!(error.to_string().contains("persistent_scrollback_lines"));
}

#[test]
fn sidebar_visible_round_trips_through_save_and_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut config = AppConfig::default();
    config.appearance.sidebar_visible = false;
    config.general.enable_pr_lookup = true;
    let toml_str = toml::to_string(&config).unwrap();
    std::fs::write(&path, toml_str).unwrap();
    let loaded = load_config_from_path(&path).unwrap();
    assert!(!loaded.appearance.sidebar_visible);
    assert!(loaded.general.enable_pr_lookup);
}

#[test]
fn save_config_to_path_replaces_existing_file_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "old = true\n").unwrap();
    let mut config = AppConfig::default();
    config.general.shell = "/bin/sh".to_string();
    config.appearance.sidebar_visible = false;

    save_config_to_path(&path, &config).unwrap();

    let loaded = load_config_from_path(&path).unwrap();
    assert!(!loaded.appearance.sidebar_visible);
    let siblings: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        siblings
            .iter()
            .all(|name| !name.to_string_lossy().contains(".tmp-")),
        "unexpected temp file sibling: {siblings:?}"
    );
}

#[cfg(unix)]
#[test]
fn save_config_to_path_creates_missing_config_directories_private() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let config_home = dir.path().join("xdg-config");
    let app_dir = config_home.join("forktty");
    let path = app_dir.join("config.toml");
    let mut config = AppConfig::default();
    config.general.shell = "/bin/sh".to_string();

    save_config_to_path(&path, &config).unwrap();

    assert_eq!(
        fs::metadata(&config_home).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&app_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[cfg(unix)]
#[test]
fn save_config_to_path_keeps_existing_directory_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let app_dir = dir.path().join("forktty");
    fs::create_dir(&app_dir).unwrap();
    fs::set_permissions(&app_dir, fs::Permissions::from_mode(0o755)).unwrap();
    let path = app_dir.join("config.toml");
    let mut config = AppConfig::default();
    config.general.shell = "/bin/sh".to_string();

    save_config_to_path(&path, &config).unwrap();

    assert_eq!(
        fs::metadata(&app_dir).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[cfg(unix)]
#[test]
fn save_config_to_path_preserves_existing_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "old = true\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut config = AppConfig::default();
    config.general.shell = "/bin/sh".to_string();

    save_config_to_path(&path, &config).unwrap();

    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[cfg(unix)]
#[test]
fn save_config_to_path_updates_symlink_target_without_replacing_link() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let link_dir = dir.path().join("config-dir");
    let managed_dir = dir.path().join("managed");
    fs::create_dir_all(&link_dir).unwrap();
    fs::create_dir_all(&managed_dir).unwrap();
    let path = link_dir.join("config.toml");
    let target = managed_dir.join("config.toml");
    fs::write(&target, "old = true\n").unwrap();
    symlink(&target, &path).unwrap();
    let mut config = AppConfig::default();
    config.general.shell = "/bin/sh".to_string();
    config.appearance.sidebar_visible = false;

    save_config_to_path(&path, &config).unwrap();

    assert!(fs::symlink_metadata(&path)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&path).unwrap(), target);
    let loaded = load_config_from_path(&path).unwrap();
    assert!(!loaded.appearance.sidebar_visible);
    let link_siblings: Vec<_> = fs::read_dir(&link_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(link_siblings, [std::ffi::OsString::from("config.toml")]);
    let managed_siblings: Vec<_> = fs::read_dir(&managed_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        managed_siblings
            .iter()
            .all(|name| !name.to_string_lossy().contains(".tmp-")),
        "unexpected temp file sibling: {managed_siblings:?}"
    );
}

#[cfg(unix)]
#[test]
fn save_config_to_path_replaces_broken_symlink_with_regular_file() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    symlink(dir.path().join("missing-target.toml"), &path).unwrap();
    let mut config = AppConfig::default();
    config.general.shell = "/bin/sh".to_string();

    save_config_to_path(&path, &config).expect("save through broken config symlink should succeed");

    assert!(
        fs::symlink_metadata(&path).unwrap().is_file(),
        "broken symlink should be replaced by a regular file"
    );
    let loaded = load_config_from_path(&path).unwrap();
    assert_eq!(loaded.general.shell, "/bin/sh");
}
#[test]
fn update_config_at_path_rebases_change_on_latest_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut config = AppConfig::default();
    config.general.shell = "/bin/sh".to_string();
    config.appearance.sidebar_visible = true;
    save_config_to_path(&path, &config).unwrap();

    let updated_shell = ["/bin/bash", "/usr/bin/bash", "/usr/bin/env", "/bin/sh"]
        .into_iter()
        .find(|candidate| {
            *candidate != config.general.shell && is_executable_file(Path::new(candidate))
        })
        .unwrap_or(&config.general.shell)
        .to_string();
    update_config_at_path(&path, |next| {
        next.general.shell = updated_shell.clone();
    })
    .unwrap();
    update_config_at_path(&path, |next| {
        next.appearance.sidebar_visible = false;
    })
    .unwrap();

    let loaded = load_config_from_path(&path).unwrap();
    assert_eq!(loaded.general.shell, updated_shell);
    assert!(!loaded.appearance.sidebar_visible);
}
