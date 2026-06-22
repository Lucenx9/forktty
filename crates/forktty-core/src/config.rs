use crate::backup::BackupReservationKind;
use crate::command_safety::{is_executable_file, is_shell_trampoline};
use serde::{Deserialize, Serialize};
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
    #[serde(default = "default_theme_source")]
    pub theme_source: String,
    #[serde(default = "default_shell")]
    pub shell: String,
    #[serde(default = "default_worktree_layout")]
    pub worktree_layout: String,
    #[serde(default)]
    pub enable_pr_lookup: bool,
    #[serde(default)]
    pub notification_command: String,
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
            theme_source: default_theme_source(),
            shell: default_shell(),
            worktree_layout: default_worktree_layout(),
            enable_pr_lookup: false,
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
    if config.general.theme_source.as_str() != "dark" {
        return Err(ConfigError::Invalid(
            "general.theme_source must be 'dark'".to_string(),
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
    config.general.theme_source = normalize_config_choice(&config.general.theme_source, &["dark"])
        .unwrap_or_else(default_theme_source);
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

fn default_theme_source() -> String {
    "dark".to_string()
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
mod tests {
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
            ["codex", "claude", "pi", "opencode", "antigravity"]
        );
        assert!(config.team.auto_fallback);
        assert!(config.team.disabled_agents.is_empty());
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
            provider_order = ["Pi", "unknown", "codex", "pi", "agy"]
            auto_fallback = false
            disabled_agents = ["open-code", "bad", "codex", "codex"]
            "#,
        )
        .unwrap();

        let config = load_config_from_path(&path).unwrap();

        assert_eq!(config.team.default_agent, "claude");
        assert_eq!(config.team.provider_order, ["pi", "codex", "antigravity"]);
        assert!(!config.team.auto_fallback);
        assert_eq!(config.team.disabled_agents, ["opencode", "codex"]);
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
    fn validate_config_rejects_invalid_theme_source() {
        let mut config = AppConfig::default();
        config.general.shell = dummy_executable_path();
        config.general.theme_source = "light".to_string();
        let err = validate_config(&config).unwrap_err();
        assert!(err
            .to_string()
            .contains("general.theme_source must be 'dark'"));
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
            theme_source = " Dark "
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

        assert_eq!(config.general.theme_source, "dark");
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
    fn saved_config_rejects_invalid_theme_source() {
        let mut config = AppConfig::default();
        config.general.shell = "/bin/sh".to_string();
        config.general.theme_source = "purple".to_string();

        let error = validate_config(&config).unwrap_err();

        assert!(error.to_string().contains("theme_source"));
    }

    #[test]
    fn loaded_config_normalizes_legacy_light_theme_to_dark() {
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

        assert_eq!(config.general.theme_source, "dark");
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
        let (recovery, quarantined_path) =
            assert_recovery_and_get_quarantined_path(config, recovery);
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

        let reserved =
            available_bad_config_path(&path, "20260521010203", BackupReservationKind::File);

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

        save_config_to_path(&path, &config)
            .expect("save through broken config symlink should succeed");

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
}
