use crate::command_safety::{is_executable_file, is_shell_trampoline};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
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
    #[serde(default = "default_terminal_theme")]
    pub terminal_theme: String,
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

pub const TERMINAL_THEME_SYSTEM: &str = "system";
pub const TERMINAL_THEME_CATPPUCCIN_MOCHA: &str = "catppuccin-mocha";
pub const TERMINAL_THEME_ROSE_PINE: &str = "rose-pine";
pub const TERMINAL_THEME_TOKYO_NIGHT: &str = "tokyo-night";
pub const TERMINAL_THEME_DRACULA: &str = "dracula";
pub const TERMINAL_THEME_GRUVBOX_DARK: &str = "gruvbox-dark";
pub const TERMINAL_THEME_CHOICES: &[&str] = &[
    TERMINAL_THEME_SYSTEM,
    TERMINAL_THEME_CATPPUCCIN_MOCHA,
    TERMINAL_THEME_ROSE_PINE,
    TERMINAL_THEME_TOKYO_NIGHT,
    TERMINAL_THEME_DRACULA,
    TERMINAL_THEME_GRUVBOX_DARK,
];

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
            terminal_theme: default_terminal_theme(),
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
    let write_path = config_write_path(path)?;
    if let Some(parent) = write_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(config)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp_path = write_path.with_extension(format!("toml.tmp-{}-{nonce}", std::process::id()));
    let result = (|| -> Result<(), ConfigError> {
        let mut tmp_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        apply_config_permissions(&write_path, &tmp_path)?;
        tmp_file.write_all(content.as_bytes())?;
        tmp_file.sync_all()?;
        fs::rename(&tmp_path, &write_path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn apply_config_permissions(write_path: &Path, tmp_path: &Path) -> Result<(), ConfigError> {
    #[cfg(unix)]
    {
        let mode = fs::metadata(write_path)
            .map(|meta| meta.permissions().mode())
            .unwrap_or(0o600);
        fs::set_permissions(tmp_path, fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

fn config_write_path(path: &Path) -> Result<PathBuf, ConfigError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => match fs::canonicalize(path) {
            Ok(resolved) => Ok(resolved),
            // Broken symlink: fall through to overwriting the dangling link
            // with the new file via the atomic tmp+rename below.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
            Err(err) => Err(err.into()),
        },
        Ok(_) => Ok(path.to_path_buf()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(err) => Err(err.into()),
    }
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
        config.general.theme_source.as_str(),
        "auto" | "light" | "dark"
    ) {
        return Err(ConfigError::Invalid(
            "general.theme_source must be one of: auto, light, dark".to_string(),
        ));
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
    if !TERMINAL_THEME_CHOICES.contains(&config.appearance.terminal_theme.as_str()) {
        return Err(ConfigError::Invalid(format!(
            "appearance.terminal_theme must be one of: {}",
            TERMINAL_THEME_CHOICES.join(", ")
        )));
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
    config.general.theme_source =
        normalize_config_choice(&config.general.theme_source, &["auto", "light", "dark"])
            .unwrap_or_else(default_theme_source);
    config.general.worktree_layout = normalize_config_choice(
        &config.general.worktree_layout,
        &["nested", "sibling", "outer-nested"],
    )
    .unwrap_or_else(default_worktree_layout);
    config.appearance.sidebar_position =
        normalize_config_choice(&config.appearance.sidebar_position, &["left", "right"])
            .unwrap_or_else(default_sidebar_position);
    config.appearance.terminal_renderer = normalize_config_choice(
        &config.appearance.terminal_renderer,
        &["auto", "dom", "canvas", "webgl", "vte"],
    )
    .unwrap_or_else(default_terminal_renderer);
    config.appearance.terminal_theme =
        normalize_config_choice(&config.appearance.terminal_theme, TERMINAL_THEME_CHOICES)
            .unwrap_or_else(default_terminal_theme);
    config.appearance.window_mode =
        normalize_config_choice(&config.appearance.window_mode, &["normal", "quake"])
            .unwrap_or_else(default_window_mode);
    if config.appearance.font_size == 0 {
        config.appearance.font_size = default_font_size();
    }
    config
}

fn normalize_config_choice(value: &str, allowed: &[&str]) -> Option<String> {
    let normalized = value.trim().to_ascii_lowercase();
    allowed.contains(&normalized.as_str()).then_some(normalized)
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
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    }
    let quarantine_path = available_bad_config_path(path, timestamp);
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
fn default_terminal_theme() -> String {
    TERMINAL_THEME_SYSTEM.to_string()
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
        assert_eq!(config.appearance.terminal_theme, TERMINAL_THEME_SYSTEM);
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
    fn loaded_config_normalizes_choice_values_from_manual_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
            [general]
            shell = "/bin/sh"
            theme_source = " Dark "
            worktree_layout = " SIBLING "

            [appearance]
            sidebar_position = " Right "
            terminal_renderer = " VTE "
            terminal_theme = " Tokyo-Night "
            window_mode = " Quake "
            "#,
        )
        .unwrap();

        let config = load_config_from_path(&path).unwrap();

        assert_eq!(config.general.theme_source, "dark");
        assert_eq!(config.general.worktree_layout, "sibling");
        assert_eq!(config.appearance.sidebar_position, "right");
        assert_eq!(config.appearance.terminal_renderer, "vte");
        assert_eq!(config.appearance.terminal_theme, TERMINAL_THEME_TOKYO_NIGHT);
        assert_eq!(config.appearance.window_mode, "quake");
    }

    #[test]
    fn saved_config_rejects_invalid_theme_source() {
        let mut config = AppConfig::default();
        config.general.shell = "/bin/sh".to_string();
        config.general.theme_source = "purple".to_string();

        let error = validate_config(&config).unwrap_err();

        assert!(error.to_string().contains("theme_source"));
    }

    #[test]
    fn saved_config_rejects_invalid_terminal_theme() {
        let mut config = AppConfig::default();
        config.general.shell = "/bin/sh".to_string();
        config.appearance.terminal_theme = "solarized".to_string();

        let error = validate_config(&config).unwrap_err();

        assert!(error.to_string().contains("terminal_theme"));
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
    fn recovery_quarantines_config_directory_and_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("note.txt"), "not a config").unwrap();

        let (config, recovery) = load_config_from_path_with_recovery(&path).unwrap();

        assert_eq!(config, AppConfig::default());
        assert!(
            !path.exists(),
            "bad config directory should be renamed aside"
        );
        let quarantined_path = recovery
            .expect("expected recovery details")
            .quarantined_path
            .expect("expected quarantined path");
        assert!(quarantined_path.is_dir());
        assert_eq!(
            fs::read_to_string(quarantined_path.join("note.txt")).unwrap(),
            "not a config"
        );
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

        save_config_to_path(&path, &config)
            .expect("save through broken config symlink should succeed");

        assert!(
            fs::symlink_metadata(&path).unwrap().is_file(),
            "broken symlink should be replaced by a regular file"
        );
        let loaded = load_config_from_path(&path).unwrap();
        assert_eq!(loaded.general.shell, "/bin/sh");
    }
}
