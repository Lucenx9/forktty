use crate::command_safety::{is_executable_file, is_shell_trampoline};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("TOML serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
    #[error("Config directory not found")]
    NoConfigDir,
    #[error("Invalid configuration: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigRecovery {
    pub reason: String,
    pub quarantined_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub notifications: NotificationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeneralConfig {
    #[serde(default = "default_theme_source")]
    pub theme_source: String,
    #[serde(default = "default_shell")]
    pub shell: String,
    #[serde(default = "default_worktree_layout")]
    pub worktree_layout: String,
    #[serde(default)]
    pub notification_command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppearanceConfig {
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: u16,
    #[serde(default = "default_scrollback_lines")]
    pub scrollback_lines: u32,
    #[serde(default = "default_terminal_audible_bell")]
    pub terminal_audible_bell: bool,
    #[serde(default = "default_sidebar_position")]
    pub sidebar_position: String,
    #[serde(default = "default_sidebar_visible")]
    pub sidebar_visible: bool,
    #[serde(default = "default_terminal_renderer")]
    pub terminal_renderer: String,
    #[serde(default = "default_window_mode")]
    pub window_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationConfig {
    #[serde(default = "default_true")]
    pub desktop: bool,
    #[serde(default = "default_true")]
    pub sound: bool,
}

const MAX_CONFIG_SIZE_BYTES: u64 = 1024 * 1024;

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            theme_source: default_theme_source(),
            shell: default_shell(),
            worktree_layout: default_worktree_layout(),
            notification_command: String::new(),
        }
    }
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            font_family: default_font_family(),
            font_size: default_font_size(),
            scrollback_lines: default_scrollback_lines(),
            terminal_audible_bell: default_terminal_audible_bell(),
            sidebar_position: default_sidebar_position(),
            sidebar_visible: default_sidebar_visible(),
            terminal_renderer: default_terminal_renderer(),
            window_mode: default_window_mode(),
        }
    }
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            desktop: true,
            sound: true,
        }
    }
}

pub fn config_dir() -> Result<PathBuf, ConfigError> {
    dirs::config_dir()
        .map(|d| d.join("forktty"))
        .ok_or(ConfigError::NoConfigDir)
}

pub fn config_path() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn load_config() -> Result<AppConfig, ConfigError> {
    load_config_from_path(&config_path()?)
}

pub fn load_config_with_recovery() -> Result<(AppConfig, Option<ConfigRecovery>), ConfigError> {
    load_config_from_path_with_recovery(&config_path()?)
}

pub fn load_config_from_path_with_recovery(
    path: &Path,
) -> Result<(AppConfig, Option<ConfigRecovery>), ConfigError> {
    match load_config_from_path(path) {
        Ok(config) => Ok((config, None)),
        Err(err) => {
            let reason = err.to_string();
            let quarantined_path = quarantine_bad_config(path)?;
            Ok((
                AppConfig::default(),
                Some(ConfigRecovery {
                    reason,
                    quarantined_path,
                }),
            ))
        }
    }
}

pub fn save_config(config: &AppConfig) -> Result<(), ConfigError> {
    save_config_to_path(&config_path()?, config)
}

pub fn save_config_to_path(path: &Path, config: &AppConfig) -> Result<(), ConfigError> {
    validate_config(config)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(config)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp_path = path.with_extension(format!("toml.tmp-{}-{nonce}", std::process::id()));
    let result = (|| -> Result<(), ConfigError> {
        let mut tmp_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        tmp_file.write_all(content.as_bytes())?;
        tmp_file.sync_all()?;
        fs::rename(&tmp_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

pub fn load_config_from_path(path: &Path) -> Result<AppConfig, ConfigError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if fs::symlink_metadata(path).is_ok() {
                return Err(ConfigError::Invalid(
                    "Config path points to a missing file".to_string(),
                ));
            }
            return Ok(AppConfig::default());
        }
        Err(err) => return Err(err.into()),
    };

    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(ConfigError::Invalid(
            "Config path is not a regular file".to_string(),
        ));
    }
    if meta.len() > MAX_CONFIG_SIZE_BYTES {
        return Err(ConfigError::Invalid("Config file is too large".to_string()));
    }

    let mut content = String::new();
    file.take(MAX_CONFIG_SIZE_BYTES)
        .read_to_string(&mut content)?;
    let config = normalize_loaded_config(toml::from_str(&content)?);
    validate_config(&config)?;
    Ok(config)
}

pub fn validate_config(config: &AppConfig) -> Result<(), ConfigError> {
    let shell = config.general.shell.trim();
    if shell.is_empty() {
        return Err(ConfigError::Invalid(
            "general.shell must not be empty".to_string(),
        ));
    }
    if !is_executable_file(Path::new(shell)) {
        return Err(ConfigError::Invalid(format!(
            "general.shell must be an absolute path to an executable file: {shell}"
        )));
    }
    if !matches!(
        config.general.worktree_layout.as_str(),
        "nested" | "sibling" | "outer-nested"
    ) {
        return Err(ConfigError::Invalid(
            "general.worktree_layout must be one of: nested, sibling, outer-nested".to_string(),
        ));
    }
    if !matches!(
        config.appearance.sidebar_position.as_str(),
        "left" | "right"
    ) {
        return Err(ConfigError::Invalid(
            "appearance.sidebar_position must be 'left' or 'right'".to_string(),
        ));
    }
    if !(8..=64).contains(&config.appearance.font_size) {
        return Err(ConfigError::Invalid(
            "appearance.font_size must be between 8 and 64".to_string(),
        ));
    }
    if config.appearance.scrollback_lines > 500_000 {
        return Err(ConfigError::Invalid(
            "appearance.scrollback_lines must be 500000 or fewer".to_string(),
        ));
    }
    if !matches!(
        config.appearance.terminal_renderer.as_str(),
        "auto" | "dom" | "canvas" | "webgl" | "vte"
    ) {
        return Err(ConfigError::Invalid(
            "appearance.terminal_renderer must be one of: auto, dom, canvas, webgl, vte"
                .to_string(),
        ));
    }
    if !matches!(config.appearance.window_mode.as_str(), "normal" | "quake") {
        return Err(ConfigError::Invalid(
            "appearance.window_mode must be one of: normal, quake".to_string(),
        ));
    }
    validate_notification_command(&config.general.notification_command)?;
    Ok(())
}

fn normalize_loaded_config(mut config: AppConfig) -> AppConfig {
    if config.general.shell.trim().is_empty() {
        config.general.shell = default_shell();
    }
    if !matches!(
        config.general.worktree_layout.as_str(),
        "nested" | "sibling" | "outer-nested"
    ) {
        config.general.worktree_layout = default_worktree_layout();
    }
    if !matches!(
        config.appearance.sidebar_position.as_str(),
        "left" | "right"
    ) {
        config.appearance.sidebar_position = default_sidebar_position();
    }
    if !matches!(
        config.appearance.terminal_renderer.as_str(),
        "auto" | "dom" | "canvas" | "webgl" | "vte"
    ) {
        config.appearance.terminal_renderer = default_terminal_renderer();
    }
    if !matches!(config.appearance.window_mode.as_str(), "normal" | "quake") {
        config.appearance.window_mode = default_window_mode();
    }
    if config.appearance.font_size == 0 {
        config.appearance.font_size = default_font_size();
    }
    config
}

fn validate_notification_command(command: &str) -> Result<(), ConfigError> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let parts = shell_words::split(trimmed)
        .map_err(|err| ConfigError::Invalid(format!("general.notification_command: {err}")))?;
    let Some(program) = parts.first() else {
        return Err(ConfigError::Invalid(
            "general.notification_command must not be empty".to_string(),
        ));
    };
    if is_shell_trampoline(program, parts.get(1).map(String::as_str)) {
        return Err(ConfigError::Invalid(
            "general.notification_command must not invoke a shell with -c".to_string(),
        ));
    }
    if !is_executable_file(Path::new(program)) {
        return Err(ConfigError::Invalid(format!(
            "general.notification_command must start with an absolute path to an executable file: {program}"
        )));
    }
    Ok(())
}

fn quarantine_bad_config(path: &Path) -> Result<Option<PathBuf>, ConfigError> {
    let timestamp = chrono::Local::now().format("%Y%m%d%H%M%S").to_string();
    quarantine_bad_config_with_timestamp(path, &timestamp)
}

fn quarantine_bad_config_with_timestamp(
    path: &Path,
    timestamp: &str,
) -> Result<Option<PathBuf>, ConfigError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let file_type = metadata.file_type();
    if !file_type.is_file() && !file_type.is_symlink() {
        return Ok(None);
    }
    let quarantine_path = available_bad_config_path(path, &timestamp);
    fs::rename(path, &quarantine_path)?;
    Ok(Some(quarantine_path))
}

fn available_bad_config_path(path: &Path, timestamp: &str) -> PathBuf {
    for suffix in std::iter::once(String::new()).chain((1u32..).map(|index| format!("-{index}"))) {
        let candidate = path.with_extension(format!("toml.bad-{timestamp}{suffix}"));
        if matches!(
            fs::symlink_metadata(&candidate),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound
        ) {
            return candidate;
        }
    }
    unreachable!("unbounded quarantine path search should always return")
}

pub fn format_config_recovery_warning(recovery: &ConfigRecovery) -> String {
    match &recovery.quarantined_path {
        Some(path) => format!(
            "Could not load config; defaults are in use. The bad config was moved to {}. {}",
            path.display(),
            recovery.reason
        ),
        None => format!(
            "Could not load config; defaults are in use. {}",
            recovery.reason
        ),
    }
}

fn default_theme_source() -> String {
    "auto".to_string()
}
fn default_shell() -> String {
    default_shell_from_env(std::env::var("SHELL").ok())
}

fn default_shell_from_env(shell_env: Option<String>) -> String {
    if let Some(shell) = shell_env
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && is_executable_file(Path::new(value)))
    {
        return shell.to_string();
    }

    ["/bin/sh", "/usr/bin/sh", "/bin/bash", "/usr/bin/bash"]
        .into_iter()
        .find(|candidate| is_executable_file(Path::new(candidate)))
        .unwrap_or("/bin/sh")
        .to_string()
}
fn default_worktree_layout() -> String {
    "nested".to_string()
}
fn default_font_family() -> String {
    String::new()
}
fn default_font_size() -> u16 {
    14
}
fn default_scrollback_lines() -> u32 {
    20_000
}
fn default_terminal_audible_bell() -> bool {
    true
}
fn default_sidebar_position() -> String {
    "left".to_string()
}
fn default_sidebar_visible() -> bool {
    true
}
fn default_terminal_renderer() -> String {
    "auto".to_string()
}
fn default_window_mode() -> String {
    "normal".to_string()
}
fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_loads_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_config_from_path(&dir.path().join("missing.toml")).unwrap();
        assert_eq!(config.appearance.font_size, 14);
        assert_eq!(config.appearance.scrollback_lines, 20_000);
        assert!(config.appearance.terminal_audible_bell);
    }

    #[test]
    fn default_shell_uses_executable_shell_env() {
        assert_eq!(
            default_shell_from_env(Some("/bin/sh".to_string())),
            "/bin/sh"
        );
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
    fn accepts_vte_renderer_for_gtk_path() {
        let mut config = AppConfig::default();
        config.general.shell = "/bin/sh".to_string();
        config.appearance.terminal_renderer = "vte".to_string();
        validate_config(&config).unwrap();
    }

    #[test]
    fn loaded_config_rejects_invalid_saved_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
            [general]
            shell = "relative-shell"
            "#,
        )
        .unwrap();

        assert!(matches!(
            load_config_from_path(&path),
            Err(ConfigError::Invalid(_))
        ));
    }

    #[test]
    fn recovery_quarantines_corrupt_config_and_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "{ not toml").unwrap();

        let (config, recovery) = load_config_from_path_with_recovery(&path).unwrap();

        assert_eq!(config, AppConfig::default());
        assert!(!path.exists(), "bad config should be renamed aside");
        let recovery = recovery.expect("expected recovery details");
        assert!(recovery.reason.contains("TOML"));
        let quarantined_path = recovery
            .quarantined_path
            .expect("expected quarantined path");
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

        assert_eq!(config, AppConfig::default());
        assert!(
            fs::symlink_metadata(&path).is_err(),
            "broken config symlink should be renamed aside"
        );
        let quarantined_path = recovery
            .expect("expected recovery details")
            .quarantined_path
            .expect("expected quarantined path");
        assert!(fs::symlink_metadata(&quarantined_path)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn recovery_does_not_overwrite_existing_quarantine_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let first_candidate = path.with_extension("toml.bad-20260521010203");
        let second_candidate = path.with_extension("toml.bad-20260521010203-1");
        fs::write(&path, "new bad config").unwrap();
        fs::write(&first_candidate, "previous bad config").unwrap();

        let quarantine_path = quarantine_bad_config_with_timestamp(&path, "20260521010203")
            .unwrap()
            .unwrap();

        assert_eq!(quarantine_path, second_candidate);
        assert!(!path.exists());
        assert_eq!(
            fs::read_to_string(&first_candidate).unwrap(),
            "previous bad config"
        );
        assert_eq!(
            fs::read_to_string(&second_candidate).unwrap(),
            "new bad config"
        );
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

    #[test]
    fn sidebar_visible_defaults_to_true_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_config_from_path(&dir.path().join("missing.toml")).unwrap();
        assert!(config.appearance.sidebar_visible);
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
    fn sidebar_visible_round_trips_through_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = AppConfig::default();
        config.appearance.sidebar_visible = false;
        let toml_str = toml::to_string(&config).unwrap();
        std::fs::write(&path, toml_str).unwrap();
        let loaded = load_config_from_path(&path).unwrap();
        assert!(!loaded.appearance.sidebar_visible);
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
}
