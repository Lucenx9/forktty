use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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

pub fn save_config(config: &AppConfig) -> Result<(), ConfigError> {
    validate_config(config)?;
    let dir = config_dir()?;
    fs::create_dir_all(&dir)?;
    fs::write(config_path()?, toml::to_string_pretty(config)?)?;
    Ok(())
}

pub fn load_config_from_path(path: &Path) -> Result<AppConfig, ConfigError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
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

fn is_shell_trampoline(program: &str, first_arg: Option<&str>) -> bool {
    if first_arg != Some("-c") {
        return false;
    }
    let shell = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    matches!(shell, "sh" | "bash" | "dash" | "zsh" | "fish" | "ksh") || shell.ends_with("sh")
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn default_theme_source() -> String {
    "auto".to_string()
}
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
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
}
