//! Config loading, validation, normalization, and private on-disk persistence.

use crate::backup::BackupReservationKind;
use crate::command_safety::{is_executable_file, is_shell_trampoline};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
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
    #[serde(default)]
    pub team: TeamConfig,
    #[serde(default)]
    pub updates: UpdateConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeneralConfig {
    #[serde(default = "default_shell")]
    pub shell: String,
    #[serde(default = "default_worktree_layout")]
    pub worktree_layout: String,
    #[serde(default)]
    pub enable_pr_lookup: bool,
    #[serde(default)]
    pub notification_command: String,
    /// Run generic terminal panes under a detach/reattach broker (`dtach`) so
    /// their process tree survives a GTK UI restart. Off by default: it changes
    /// how shells are launched and requires a broker on `PATH`. Agent, SSH, and
    /// browser surfaces are unaffected. See [`crate::pty_persistence`].
    #[serde(default)]
    pub persist_terminal_processes: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppearanceConfig {
    #[serde(default = "default_font_family", skip_serializing)]
    pub font_family: String,
    #[serde(default = "default_font_size", skip_serializing)]
    pub font_size: u16,
    #[serde(default = "default_scrollback_lines", skip_serializing)]
    pub scrollback_lines: u32,
    #[serde(default)]
    pub persistent_scrollback_lines: u32,
    #[serde(default = "default_terminal_audible_bell", skip_serializing)]
    pub terminal_audible_bell: bool,
    #[serde(default = "default_sidebar_position")]
    pub sidebar_position: String,
    #[serde(default = "default_sidebar_visible")]
    pub sidebar_visible: bool,
    #[serde(default = "default_terminal_renderer", skip_serializing)]
    pub terminal_renderer: String,
    #[serde(default = "default_terminal_theme", skip_serializing)]
    pub terminal_theme: String,
    #[serde(default = "default_window_mode")]
    pub window_mode: String,
    /// Legacy alpha key kept only so older config files load. Terminal panes
    /// always use the embedded Ghostty GTK widget in current builds, and new
    /// saves omit this non-functional switch.
    #[serde(default = "default_true", skip_serializing)]
    pub embedded_ghostty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationConfig {
    #[serde(default = "default_true")]
    pub desktop: bool,
    #[serde(default = "default_true")]
    pub sound: bool,
    #[serde(default)]
    pub blocked_terminal_apps: Vec<String>,
    #[serde(default)]
    pub blocked_terminal_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TeamConfig {
    #[serde(default = "default_team_default_agent")]
    pub default_agent: String,
    #[serde(default = "default_team_provider_order")]
    pub provider_order: Vec<String>,
    #[serde(default = "default_true")]
    pub auto_fallback: bool,
    #[serde(default)]
    pub disabled_agents: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_commands: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpdateConfig {
    #[serde(default = "default_true")]
    pub auto_check: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TelemetryConfig {
    #[serde(default = "default_true")]
    pub anonymous_ping: bool,
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
pub const TEAM_AGENT_AUTO: &str = "auto";
pub const TEAM_PROVIDER_CHOICES: &[&str] = &["codex", "claude", "pi", "opencode", "antigravity"];
pub const MAX_PERSISTENT_SCROLLBACK_LINES: u32 = 1_000;

const MAX_CONFIG_SIZE_BYTES: u64 = 1024 * 1024;
const MAX_NOTIFICATION_FILTER_VALUES: usize = 64;
const MAX_NOTIFICATION_FILTER_VALUE_CHARS: usize = 120;

static CONFIG_UPDATE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            shell: default_shell(),
            worktree_layout: default_worktree_layout(),
            enable_pr_lookup: false,
            notification_command: String::new(),
            persist_terminal_processes: false,
        }
    }
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            font_family: default_font_family(),
            font_size: default_font_size(),
            scrollback_lines: default_scrollback_lines(),
            persistent_scrollback_lines: 0,
            terminal_audible_bell: default_terminal_audible_bell(),
            sidebar_position: default_sidebar_position(),
            sidebar_visible: default_sidebar_visible(),
            terminal_renderer: default_terminal_renderer(),
            terminal_theme: default_terminal_theme(),
            window_mode: default_window_mode(),
            embedded_ghostty: default_true(),
        }
    }
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            desktop: true,
            sound: true,
            blocked_terminal_apps: Vec::new(),
            blocked_terminal_types: Vec::new(),
        }
    }
}

impl Default for TeamConfig {
    fn default() -> Self {
        Self {
            default_agent: default_team_default_agent(),
            provider_order: default_team_provider_order(),
            auto_fallback: true,
            disabled_agents: Vec::new(),
            provider_commands: BTreeMap::new(),
        }
    }
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self { auto_check: true }
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            anonymous_ping: true,
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
        Err(err) if should_quarantine_config_load_error(&err) => {
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
        Err(err) => Err(err),
    }
}

pub fn save_config(config: &AppConfig) -> Result<(), ConfigError> {
    save_config_to_path(&config_path()?, config)
}

pub fn update_config<F>(update: F) -> Result<AppConfig, ConfigError>
where
    F: FnOnce(&mut AppConfig),
{
    update_config_at_path(&config_path()?, update)
}

pub fn update_config_at_path<F>(path: &Path, update: F) -> Result<AppConfig, ConfigError>
where
    F: FnOnce(&mut AppConfig),
{
    update_config_at_path_if_changed(path, update).map(|(config, _changed)| config)
}

pub fn update_config_if_changed<F>(update: F) -> Result<(AppConfig, bool), ConfigError>
where
    F: FnOnce(&mut AppConfig),
{
    update_config_at_path_if_changed(&config_path()?, update)
}

pub fn update_config_at_path_if_changed<F>(
    path: &Path,
    update: F,
) -> Result<(AppConfig, bool), ConfigError>
where
    F: FnOnce(&mut AppConfig),
{
    let _guard = CONFIG_UPDATE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let mut next = load_config_from_path(path)?;
    let previous = next.clone();
    update(&mut next);
    if next == previous {
        return Ok((next, false));
    }
    save_config_to_path_unlocked(path, &next)?;
    Ok((next, true))
}

pub fn save_config_to_path(path: &Path, config: &AppConfig) -> Result<(), ConfigError> {
    let _guard = CONFIG_UPDATE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    save_config_to_path_unlocked(path, config)
}

fn save_config_to_path_unlocked(path: &Path, config: &AppConfig) -> Result<(), ConfigError> {
    validate_config(config)?;
    let write_path = config_write_path(path)?;
    if let Some(parent) = write_path.parent() {
        ensure_config_parent_dir(parent)?;
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
        sync_parent_dir(&write_path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn ensure_config_parent_dir(parent: &Path) -> Result<(), ConfigError> {
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        ensure_config_parent_dir_unix(parent)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(parent)?;
        Ok(())
    }
}

#[cfg(unix)]
fn ensure_config_parent_dir_unix(parent: &Path) -> Result<(), ConfigError> {
    let mut missing = Vec::new();
    let mut cursor = Some(parent);
    while let Some(dir) = cursor {
        match fs::metadata(dir) {
            Ok(meta) if meta.is_dir() => break,
            Ok(_) => {
                return Err(ConfigError::Invalid(format!(
                    "Config directory path is not a directory: {}",
                    dir.display()
                )));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                missing.push(dir.to_path_buf());
                cursor = dir.parent().filter(|path| !path.as_os_str().is_empty());
            }
            Err(err) => return Err(err.into()),
        }
    }

    for dir in missing.iter().rev() {
        match fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if !fs::metadata(dir)?.is_dir() {
                    return Err(ConfigError::Invalid(format!(
                        "Config directory path is not a directory: {}",
                        dir.display()
                    )));
                }
            }
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

fn sync_parent_dir(path: &Path) -> Result<(), ConfigError> {
    #[cfg(unix)]
    {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::File::open(parent)?.sync_all()?;
        }
    }
    Ok(())
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
    let file = match open_config_for_read(path) {
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
    file.take(MAX_CONFIG_SIZE_BYTES + 1)
        .read_to_string(&mut content)?;
    // Guard against the file growing between the metadata check and the read:
    // `take(MAX)` would silently truncate and feed a partial TOML document to
    // the parser. Reject instead of parsing a truncated config.
    if content.len() as u64 > MAX_CONFIG_SIZE_BYTES {
        return Err(ConfigError::Invalid("Config file is too large".to_string()));
    }
    let config = normalize_loaded_config(toml::from_str(&content)?);
    validate_config(&config)?;
    Ok(config)
}

fn open_config_for_read(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NONBLOCK);
    }
    options.open(path)
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
    if config.appearance.scrollback_lines > 500_000 {
        return Err(ConfigError::Invalid(
            "appearance.scrollback_lines must be 500000 or fewer".to_string(),
        ));
    }
    if config.appearance.persistent_scrollback_lines > MAX_PERSISTENT_SCROLLBACK_LINES {
        return Err(ConfigError::Invalid(format!(
            "appearance.persistent_scrollback_lines must be {MAX_PERSISTENT_SCROLLBACK_LINES} or fewer"
        )));
    }
    if !matches!(
        config.appearance.terminal_renderer.as_str(),
        "auto" | "dom" | "canvas" | "webgl" | "ghostty" | "vte"
    ) {
        return Err(ConfigError::Invalid(
            "appearance.terminal_renderer must be one of: auto, dom, canvas, webgl, ghostty, vte"
                .to_string(),
        ));
    }
    if !matches!(config.appearance.window_mode.as_str(), "normal" | "quake") {
        return Err(ConfigError::Invalid(
            "appearance.window_mode must be one of: normal, quake".to_string(),
        ));
    }
    validate_notification_filter_values(
        "notifications.blocked_terminal_apps",
        &config.notifications.blocked_terminal_apps,
    )?;
    validate_notification_filter_values(
        "notifications.blocked_terminal_types",
        &config.notifications.blocked_terminal_types,
    )?;
    validate_notification_command(&config.general.notification_command)?;
    validate_team_config(&config.team)?;
    Ok(())
}

fn normalize_loaded_config(mut config: AppConfig) -> AppConfig {
    // Whether the configured shell exists is environment-dependent (deleted
    // shell, config copied from another machine), so fall back to the default
    // shell on load instead of failing validation and quarantining the whole
    // config.
    let shell = config.general.shell.trim();
    if shell.is_empty() || !is_executable_file(Path::new(shell)) {
        config.general.shell = default_shell();
    } else {
        config.general.shell = shell.to_string();
    }
    config.general.worktree_layout = normalize_config_choice(
        &config.general.worktree_layout,
        &["nested", "sibling", "outer-nested"],
    )
    .unwrap_or_else(default_worktree_layout);
    config.general.notification_command = config.general.notification_command.trim().to_string();
    // Drop a notification command whose program is missing or not executable:
    // like the shell above, that is environment-dependent, so normalize on load
    // instead of quarantining the whole config. Structural problems
    // (unparseable quoting, shell `-c` trampolines) still fail validation and
    // quarantine.
    if let Ok(parts) = shell_words::split(&config.general.notification_command) {
        match parts.first() {
            Some(program) => {
                if !is_shell_trampoline(program, &parts[1..])
                    && !is_executable_file(Path::new(program))
                {
                    config.general.notification_command = String::new();
                }
            }
            // No tokens at all (e.g. an inline shell comment like
            // `notification_command = "# disabled"`) is benign — it is
            // equivalent to "no command", so normalize it to empty instead of
            // letting validation reject it and quarantine the whole config.
            None => config.general.notification_command = String::new(),
        }
    }
    config.appearance.sidebar_position =
        normalize_config_choice(&config.appearance.sidebar_position, &["left", "right"])
            .unwrap_or_else(default_sidebar_position);
    config.appearance.terminal_renderer =
        normalize_terminal_renderer_choice(&config.appearance.terminal_renderer)
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
    // Clamp hand-edited numeric ranges instead of quarantining the entire
    // config on load. `font_size` is a legacy field ignored by the GTK
    // renderer, but keeping it normalized makes old config round trips stable.
    config.appearance.font_size = config.appearance.font_size.clamp(8, 64);
    config.appearance.scrollback_lines = config.appearance.scrollback_lines.min(500_000);
    config.appearance.persistent_scrollback_lines = config
        .appearance
        .persistent_scrollback_lines
        .min(MAX_PERSISTENT_SCROLLBACK_LINES);
    config.notifications.blocked_terminal_apps =
        normalize_notification_filter_values(config.notifications.blocked_terminal_apps);
    config.notifications.blocked_terminal_types =
        normalize_notification_filter_values(config.notifications.blocked_terminal_types);
    config.team = normalize_team_config(config.team);
    config
}

fn normalize_team_config(mut team: TeamConfig) -> TeamConfig {
    team.default_agent =
        normalize_team_agent_choice(&team.default_agent).unwrap_or_else(default_team_default_agent);
    team.provider_order = normalize_team_provider_list(team.provider_order);
    if team.provider_order.is_empty() {
        team.provider_order = default_team_provider_order();
    }
    team.disabled_agents = normalize_team_provider_list(team.disabled_agents);
    team.provider_commands = normalize_team_provider_commands(team.provider_commands);
    if team.default_agent != TEAM_AGENT_AUTO && team.disabled_agents.contains(&team.default_agent) {
        team.default_agent = TEAM_AGENT_AUTO.to_string();
    }
    team
}

fn normalize_team_provider_list(values: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for value in values {
        let Some(agent) = canonical_team_provider(&value) else {
            continue;
        };
        if !normalized.iter().any(|item| item == agent) {
            normalized.push(agent.to_string());
        }
    }
    normalized
}

fn normalize_team_provider_commands(
    commands: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    commands
        .into_iter()
        .filter_map(|(provider, command)| {
            let provider = canonical_team_provider(&provider)?;
            let command = command.trim();
            (!command.is_empty()).then(|| (provider.to_string(), command.to_string()))
        })
        .collect()
}

pub fn normalize_team_agent_choice(value: &str) -> Option<String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized == TEAM_AGENT_AUTO {
        return Some(TEAM_AGENT_AUTO.to_string());
    }
    canonical_team_provider(&normalized).map(str::to_string)
}

pub fn canonical_team_provider(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "codex" => Some("codex"),
        "claude" | "claude_code" | "claude-code" => Some("claude"),
        "opencode" | "open_code" | "open-code" => Some("opencode"),
        "antigravity" | "agy" => Some("antigravity"),
        "pi" => Some("pi"),
        _ => None,
    }
}

pub fn team_provider_program(provider: &str) -> Option<&'static str> {
    match canonical_team_provider(provider)? {
        "codex" => Some("codex"),
        "claude" => Some("claude"),
        "pi" => Some("pi"),
        "opencode" => Some("opencode"),
        "antigravity" => Some("agy"),
        _ => None,
    }
}

pub fn team_provider_command<'a>(team: &'a TeamConfig, provider: &str) -> Option<&'a str> {
    let provider = canonical_team_provider(provider)?;
    team.provider_commands.get(provider).map(String::as_str)
}

pub fn validate_team_provider_command_value(command: &str) -> Result<(), String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("must not be empty".to_string());
    }
    if command.len() > 4096 {
        return Err("must be 4096 bytes or fewer".to_string());
    }
    if command.chars().any(char::is_control) {
        return Err("must not contain control characters".to_string());
    }
    let path = Path::new(command);
    if path.is_absolute() {
        return Ok(());
    }
    if command.starts_with('-') {
        return Err("must not be flag-like".to_string());
    }
    if command.chars().any(char::is_whitespace) {
        return Err(
            "must be a bare command name or absolute path; arguments belong in the launch args"
                .to_string(),
        );
    }
    if path.components().count() > 1 {
        return Err("must be a bare command name or absolute path".to_string());
    }
    Ok(())
}

fn normalize_notification_filter_values(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .filter_map(|value| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        })
        .collect()
}

fn normalize_config_choice(value: &str, allowed: &[&str]) -> Option<String> {
    let normalized = value.trim().to_ascii_lowercase();
    allowed.contains(&normalized.as_str()).then_some(normalized)
}

fn normalize_terminal_renderer_choice(value: &str) -> Option<String> {
    normalize_config_choice(value, &["auto", "dom", "canvas", "webgl", "ghostty", "vte"]).map(
        |choice| {
            if choice == "vte" {
                "auto".to_string()
            } else {
                choice
            }
        },
    )
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
    if is_shell_trampoline(program, &parts[1..]) {
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

fn validate_team_config(team: &TeamConfig) -> Result<(), ConfigError> {
    if normalize_team_agent_choice(&team.default_agent).as_deref() != Some(&team.default_agent) {
        return Err(ConfigError::Invalid(
            "team.default_agent must be auto, codex, claude, pi, opencode, or antigravity"
                .to_string(),
        ));
    }
    validate_team_provider_list("team.provider_order", &team.provider_order, true)?;
    validate_team_provider_list("team.disabled_agents", &team.disabled_agents, false)?;
    validate_team_provider_commands(&team.provider_commands)?;
    if team.default_agent != TEAM_AGENT_AUTO && team.disabled_agents.contains(&team.default_agent) {
        return Err(ConfigError::Invalid(
            "team.default_agent must not also appear in team.disabled_agents".to_string(),
        ));
    }
    Ok(())
}

fn validate_team_provider_list(
    name: &str,
    values: &[String],
    require_non_empty: bool,
) -> Result<(), ConfigError> {
    if require_non_empty && values.is_empty() {
        return Err(ConfigError::Invalid(format!("{name} must not be empty")));
    }
    if values.len() > TEAM_PROVIDER_CHOICES.len() {
        return Err(ConfigError::Invalid(format!(
            "{name} must contain {} entries or fewer",
            TEAM_PROVIDER_CHOICES.len()
        )));
    }
    let mut seen = Vec::new();
    for value in values {
        if canonical_team_provider(value) != Some(value.as_str()) {
            return Err(ConfigError::Invalid(format!(
                "{name} entries must be canonical provider names: codex, claude, pi, opencode, antigravity"
            )));
        }
        if seen
            .iter()
            .any(|item: &&String| item.as_str() == value.as_str())
        {
            return Err(ConfigError::Invalid(format!(
                "{name} entries must not contain duplicates"
            )));
        }
        seen.push(value);
    }
    Ok(())
}

fn validate_team_provider_commands(commands: &BTreeMap<String, String>) -> Result<(), ConfigError> {
    for (provider, command) in commands {
        if canonical_team_provider(provider) != Some(provider.as_str()) {
            return Err(ConfigError::Invalid(
                "team.provider_commands keys must be canonical provider names: codex, claude, pi, opencode, antigravity"
                    .to_string(),
            ));
        }
        if let Err(err) = validate_team_provider_command_value(command) {
            return Err(ConfigError::Invalid(format!(
                "team.provider_commands.{provider} {err}"
            )));
        }
    }
    Ok(())
}

fn validate_notification_filter_values(name: &str, values: &[String]) -> Result<(), ConfigError> {
    if values.len() > MAX_NOTIFICATION_FILTER_VALUES {
        return Err(ConfigError::Invalid(format!(
            "{name} must contain {MAX_NOTIFICATION_FILTER_VALUES} values or fewer"
        )));
    }
    for value in values {
        if value.trim().is_empty()
            || value.trim() != value
            || value.chars().count() > MAX_NOTIFICATION_FILTER_VALUE_CHARS
        {
            return Err(ConfigError::Invalid(format!(
                "{name} entries must be non-empty, trimmed, and {MAX_NOTIFICATION_FILTER_VALUE_CHARS} characters or fewer"
            )));
        }
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
    let reservation_kind = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => BackupReservationKind::Directory,
        Ok(_) => BackupReservationKind::File,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let quarantine_path = available_bad_config_path(path, timestamp, reservation_kind);
    fs::rename(path, &quarantine_path)?;
    sync_parent_dir(&quarantine_path)?;
    Ok(Some(quarantine_path))
}

fn should_quarantine_config_load_error(err: &ConfigError) -> bool {
    match err {
        ConfigError::TomlParse(_) | ConfigError::Invalid(_) => true,
        ConfigError::Io(err) if err.kind() == std::io::ErrorKind::InvalidData => true,
        ConfigError::Io(_) | ConfigError::TomlSerialize(_) | ConfigError::NoConfigDir => false,
    }
}

fn available_bad_config_path(path: &Path, timestamp: &str, kind: BackupReservationKind) -> PathBuf {
    let extension = format!("toml.bad-{timestamp}");
    crate::backup::reserve_unique_backup_path_with_kind(path, &extension, kind)
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
fn default_team_default_agent() -> String {
    TEAM_AGENT_AUTO.to_string()
}
pub fn default_team_provider_order() -> Vec<String> {
    TEAM_PROVIDER_CHOICES
        .iter()
        .map(|provider| (*provider).to_string())
        .collect()
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
mod tests;
