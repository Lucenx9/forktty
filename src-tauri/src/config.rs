use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
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
    NoCfgDir,
    #[error("Invalid configuration: {0}")]
    Invalid(String),
}

// --- ForkTTY config (TOML) ---

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub notifications: NotificationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceConfig {
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: u16,
    #[serde(default = "default_sidebar_position")]
    pub sidebar_position: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationConfig {
    #[serde(default = "default_true")]
    pub desktop: bool,
    #[serde(default = "default_true")]
    pub sound: bool,
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
fn default_true() -> bool {
    true
}

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

/// Get ForkTTY config directory (~/.config/forktty/)
fn config_dir() -> Result<PathBuf, ConfigError> {
    dirs::config_dir()
        .map(|d| d.join("forktty"))
        .ok_or(ConfigError::NoCfgDir)
}

/// Get ForkTTY config file path
fn config_path() -> Result<PathBuf, ConfigError> {
    Ok(config_dir()?.join("config.toml"))
}

/// Load ForkTTY config, returning defaults if file doesn't exist.
pub fn load_config() -> Result<AppConfig, ConfigError> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let content = fs::read_to_string(&path)?;
    let config: AppConfig = toml::from_str(&content)?;
    Ok(normalize_loaded_config(config))
}

/// Save ForkTTY config to disk.
pub fn save_config(config: &AppConfig) -> Result<(), ConfigError> {
    validate_config(config)?;
    let dir = config_dir()?;
    fs::create_dir_all(&dir)?;
    let content = toml::to_string_pretty(config)?;
    fs::write(config_path()?, content)?;
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
    if config.appearance.font_size == 0 {
        config.appearance.font_size = default_font_size();
    }
    config
}

fn validate_config(config: &AppConfig) -> Result<(), ConfigError> {
    let shell = config.general.shell.trim();
    if shell.is_empty() {
        return Err(ConfigError::Invalid(
            "general.shell must not be empty".to_string(),
        ));
    }
    let shell_path = Path::new(shell);
    if !shell_path.is_absolute() || !shell_path.exists() {
        return Err(ConfigError::Invalid(format!(
            "general.shell must be an absolute path to an existing file: {shell}"
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

    validate_notification_command(&config.general.notification_command)?;

    Ok(())
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

    let program_path = Path::new(program);
    if !program_path.is_absolute() || !program_path.exists() {
        return Err(ConfigError::Invalid(format!(
            "general.notification_command must start with an absolute path to an existing file: {program}"
        )));
    }

    Ok(())
}

// --- Ghostty theme parsing ---

/// Parsed terminal theme colors.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TerminalTheme {
    pub background: Option<String>,
    pub foreground: Option<String>,
    pub cursor: Option<String>,
    pub selection_background: Option<String>,
    pub selection_foreground: Option<String>,
    pub black: Option<String>,
    pub red: Option<String>,
    pub green: Option<String>,
    pub yellow: Option<String>,
    pub blue: Option<String>,
    pub magenta: Option<String>,
    pub cyan: Option<String>,
    pub white: Option<String>,
    pub bright_black: Option<String>,
    pub bright_red: Option<String>,
    pub bright_green: Option<String>,
    pub bright_yellow: Option<String>,
    pub bright_blue: Option<String>,
    pub bright_magenta: Option<String>,
    pub bright_cyan: Option<String>,
    pub bright_white: Option<String>,
    pub font_family: Option<String>,
    pub font_size: Option<u16>,
}

/// Parse a Ghostty config file (key = value format).
fn parse_ghostty_file(path: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return map,
    };

    for line in content.lines() {
        let line = line.trim();
        // Skip comments and empty lines
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    map
}

fn normalize_font_family(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() < 2 {
        return trimmed.to_string();
    }

    let bytes = trimmed.as_bytes();
    let first = bytes[0] as char;
    let last = bytes[trimmed.len() - 1] as char;
    if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
        trimmed[1..trimmed.len() - 1].trim().to_string()
    } else {
        trimmed.to_string()
    }
}

/// Extract a TerminalTheme from parsed Ghostty key-value pairs.
/// Note: palette colors must be applied separately via `apply_palette` + `parse_palette_from_content`,
/// because Ghostty uses duplicate `palette` keys that a HashMap cannot represent.
fn theme_from_ghostty_map(map: &HashMap<String, String>) -> TerminalTheme {
    TerminalTheme {
        background: map.get("background").cloned(),
        foreground: map.get("foreground").cloned(),
        cursor: map.get("cursor-color").cloned(),
        selection_background: map.get("selection-background").cloned(),
        selection_foreground: map.get("selection-foreground").cloned(),
        font_family: map
            .get("font-family")
            .map(|value| normalize_font_family(value)),
        font_size: map.get("font-size").and_then(|s| s.parse().ok()),
        ..Default::default()
    }
}

/// Parse palette entries from raw Ghostty config content (handles duplicate `palette` keys).
fn parse_palette_from_content(content: &str) -> HashMap<u8, String> {
    let mut palette = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if key == "palette" {
                let value = value.trim();
                if let Some((idx_str, color)) = value.split_once('=') {
                    if let Ok(idx) = idx_str.trim().parse::<u8>() {
                        palette.insert(idx, color.trim().to_string());
                    }
                }
            }
        }
    }
    palette
}

/// Validate a Ghostty theme name: only alphanumeric, dash, underscore, space allowed.
/// Rejects path traversal characters (/, ..) and null bytes.
fn is_valid_theme_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == ' ')
}

/// Load Ghostty theme from config and optional theme file.
fn load_ghostty_theme() -> TerminalTheme {
    let ghostty_dir = dirs::config_dir()
        .map(|d| d.join("ghostty"))
        .unwrap_or_default();

    let config_path = ghostty_dir.join("config");
    let map = parse_ghostty_file(&config_path);

    // If a theme is referenced, load the theme file first as base
    let mut theme = if let Some(theme_name) = map.get("theme") {
        if !is_valid_theme_name(theme_name) {
            return TerminalTheme::default();
        }
        let theme_file = ghostty_dir.join("themes").join(theme_name);
        if theme_file.exists() {
            let theme_map = parse_ghostty_file(&theme_file);
            let content = fs::read_to_string(&theme_file).unwrap_or_default();
            let palette = parse_palette_from_content(&content);
            let mut t = theme_from_ghostty_map(&theme_map);
            // Apply palette entries from theme file
            apply_palette(&mut t, &palette);
            t
        } else {
            // Also check system-wide Ghostty themes
            let sys_theme = PathBuf::from("/usr/share/ghostty/themes").join(theme_name);
            if sys_theme.exists() {
                let theme_map = parse_ghostty_file(&sys_theme);
                let content = fs::read_to_string(&sys_theme).unwrap_or_default();
                let palette = parse_palette_from_content(&content);
                let mut t = theme_from_ghostty_map(&theme_map);
                apply_palette(&mut t, &palette);
                t
            } else {
                TerminalTheme::default()
            }
        }
    } else {
        TerminalTheme::default()
    };

    // Override with entries from main config (config overrides theme file)
    let content = fs::read_to_string(&config_path).unwrap_or_default();
    let palette = parse_palette_from_content(&content);
    apply_palette(&mut theme, &palette);

    // Non-palette overrides
    if let Some(v) = map.get("background") {
        theme.background = Some(v.clone());
    }
    if let Some(v) = map.get("foreground") {
        theme.foreground = Some(v.clone());
    }
    if let Some(v) = map.get("cursor-color") {
        theme.cursor = Some(v.clone());
    }
    if let Some(v) = map.get("selection-background") {
        theme.selection_background = Some(v.clone());
    }
    if let Some(v) = map.get("selection-foreground") {
        theme.selection_foreground = Some(v.clone());
    }
    if let Some(v) = map.get("font-family") {
        theme.font_family = Some(normalize_font_family(v));
    }
    if let Some(v) = map.get("font-size") {
        if let Ok(size) = v.parse::<u16>() {
            theme.font_size = Some(size);
        }
    }

    theme
}

fn apply_palette(theme: &mut TerminalTheme, palette: &HashMap<u8, String>) {
    let fields: &mut [(&mut Option<String>, u8)] = &mut [
        (&mut theme.black, 0),
        (&mut theme.red, 1),
        (&mut theme.green, 2),
        (&mut theme.yellow, 3),
        (&mut theme.blue, 4),
        (&mut theme.magenta, 5),
        (&mut theme.cyan, 6),
        (&mut theme.white, 7),
        (&mut theme.bright_black, 8),
        (&mut theme.bright_red, 9),
        (&mut theme.bright_green, 10),
        (&mut theme.bright_yellow, 11),
        (&mut theme.bright_blue, 12),
        (&mut theme.bright_magenta, 13),
        (&mut theme.bright_cyan, 14),
        (&mut theme.bright_white, 15),
    ];
    for (field, idx) in fields {
        if let Some(c) = palette.get(idx) {
            **field = Some(c.clone());
        }
    }
}

/// Resolve the full terminal theme based on config.
/// If theme_source = "auto", reads Ghostty config.
/// AppConfig appearance settings override everything.
pub fn resolve_theme(config: &AppConfig) -> TerminalTheme {
    let mut theme = if config.general.theme_source == "auto" {
        load_ghostty_theme()
    } else {
        // Built-in Catppuccin Mocha as fallback
        default_catppuccin_mocha()
    };

    // AppConfig appearance overrides
    if !config.appearance.font_family.is_empty() {
        theme.font_family = Some(normalize_font_family(&config.appearance.font_family));
    }
    if config.appearance.font_size > 0 {
        theme.font_size = Some(config.appearance.font_size);
    }

    // Fill in any missing values with Catppuccin Mocha defaults
    let defaults = default_catppuccin_mocha();
    theme.background = theme.background.or(defaults.background);
    theme.foreground = theme.foreground.or(defaults.foreground);
    theme.cursor = theme.cursor.or(defaults.cursor);
    theme.selection_background = theme.selection_background.or(defaults.selection_background);
    theme.black = theme.black.or(defaults.black);
    theme.red = theme.red.or(defaults.red);
    theme.green = theme.green.or(defaults.green);
    theme.yellow = theme.yellow.or(defaults.yellow);
    theme.blue = theme.blue.or(defaults.blue);
    theme.magenta = theme.magenta.or(defaults.magenta);
    theme.cyan = theme.cyan.or(defaults.cyan);
    theme.white = theme.white.or(defaults.white);
    theme.bright_black = theme.bright_black.or(defaults.bright_black);
    theme.bright_red = theme.bright_red.or(defaults.bright_red);
    theme.bright_green = theme.bright_green.or(defaults.bright_green);
    theme.bright_yellow = theme.bright_yellow.or(defaults.bright_yellow);
    theme.bright_blue = theme.bright_blue.or(defaults.bright_blue);
    theme.bright_magenta = theme.bright_magenta.or(defaults.bright_magenta);
    theme.bright_cyan = theme.bright_cyan.or(defaults.bright_cyan);
    theme.bright_white = theme.bright_white.or(defaults.bright_white);
    theme.font_family = theme.font_family.or(defaults.font_family);
    theme.font_size = theme.font_size.or(defaults.font_size);

    theme
}

/// Catppuccin Mocha — the default dark theme (matches current hardcoded colors).
fn default_catppuccin_mocha() -> TerminalTheme {
    TerminalTheme {
        background: Some("#1e1e2e".to_string()),
        foreground: Some("#cdd6f4".to_string()),
        cursor: Some("#f5e0dc".to_string()),
        selection_background: Some("#585b70".to_string()),
        selection_foreground: None,
        black: Some("#45475a".to_string()),
        red: Some("#f38ba8".to_string()),
        green: Some("#a6e3a1".to_string()),
        yellow: Some("#f9e2af".to_string()),
        blue: Some("#89b4fa".to_string()),
        magenta: Some("#f5c2e7".to_string()),
        cyan: Some("#94e2d5".to_string()),
        white: Some("#bac2de".to_string()),
        bright_black: Some("#585b70".to_string()),
        bright_red: Some("#f38ba8".to_string()),
        bright_green: Some("#a6e3a1".to_string()),
        bright_yellow: Some("#f9e2af".to_string()),
        bright_blue: Some("#89b4fa".to_string()),
        bright_magenta: Some("#f5c2e7".to_string()),
        bright_cyan: Some("#94e2d5".to_string()),
        bright_white: Some("#a6adc8".to_string()),
        font_family: Some("monospace".to_string()),
        font_size: Some(14),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.general.theme_source, "auto");
        assert_eq!(config.appearance.font_size, 14);
        assert!(config.notifications.desktop);
    }

    #[test]
    fn test_parse_ghostty_content() {
        let content = "background = #303446\nforeground = #c6d0f5\n# comment\npalette = 0=#51576d\npalette = 1=#e78284\nfont-size = 16\n";
        let palette = parse_palette_from_content(content);
        assert_eq!(palette.get(&0), Some(&"#51576d".to_string()));
        assert_eq!(palette.get(&1), Some(&"#e78284".to_string()));
    }

    #[test]
    fn test_resolve_theme_defaults() {
        let config = AppConfig {
            general: GeneralConfig {
                theme_source: "builtin".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let theme = resolve_theme(&config);
        assert_eq!(theme.background, Some("#1e1e2e".to_string()));
        assert_eq!(theme.font_size, Some(14));
    }

    #[test]
    fn normalize_font_family_strips_wrapping_quotes() {
        assert_eq!(
            normalize_font_family("\"JetBrains Mono\""),
            "JetBrains Mono"
        );
        assert_eq!(normalize_font_family("'JetBrains Mono'"), "JetBrains Mono");
    }

    #[test]
    fn test_config_roundtrip() {
        let config = AppConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.general.theme_source, config.general.theme_source);
        assert_eq!(parsed.appearance.font_size, config.appearance.font_size);
    }

    // --- Test 2: Ghostty theme name validation ---

    #[test]
    fn theme_name_with_slash_is_rejected() {
        assert!(!is_valid_theme_name("../../etc/passwd"));
        assert!(!is_valid_theme_name("some/path"));
    }

    #[test]
    fn theme_name_with_dotdot_is_rejected() {
        // ".." contains dots; dots are not alphanumeric/dash/underscore/space
        assert!(!is_valid_theme_name(".."));
        assert!(!is_valid_theme_name("..secret"));
    }

    #[test]
    fn theme_name_with_null_byte_is_rejected() {
        assert!(!is_valid_theme_name("theme\0name"));
    }

    #[test]
    fn valid_theme_name_catppuccin_mocha_passes() {
        assert!(is_valid_theme_name("catppuccin-mocha"));
    }

    #[test]
    fn theme_name_with_space_passes() {
        assert!(is_valid_theme_name("My Theme"));
    }

    #[test]
    fn theme_name_with_underscore_passes() {
        assert!(is_valid_theme_name("dracula_pro"));
    }

    #[test]
    fn empty_theme_name_is_rejected() {
        assert!(!is_valid_theme_name(""));
    }

    #[test]
    fn validate_config_rejects_invalid_worktree_layout() {
        let mut config = AppConfig::default();
        config.general.worktree_layout = "invalid".to_string();
        let err = validate_config(&config).unwrap_err().to_string();
        assert!(err.contains("worktree_layout"));
    }

    #[test]
    fn validate_config_rejects_relative_notification_command() {
        let mut config = AppConfig::default();
        config.general.notification_command = "notify-send done".to_string();
        let err = validate_config(&config).unwrap_err().to_string();
        assert!(err.contains("notification_command"));
    }

    #[test]
    fn normalize_loaded_config_repairs_invalid_enums() {
        let mut config = AppConfig::default();
        config.general.worktree_layout = "bogus".to_string();
        config.appearance.sidebar_position = "middle".to_string();
        config.appearance.font_size = 0;
        let normalized = normalize_loaded_config(config);
        assert_eq!(normalized.general.worktree_layout, "nested");
        assert_eq!(normalized.appearance.sidebar_position, "left");
        assert_eq!(normalized.appearance.font_size, 14);
    }
}
