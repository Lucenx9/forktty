use super::*;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

thread_local! {
    // A single display-global provider, reused across spawns and settings changes.
    // Adding a fresh provider on every call would accumulate stale rules on the display.
    static TERMINAL_CSS_PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
}

const TERMINAL_ZOOM_BASE_PT: i32 = 12;
const TERMINAL_ZOOM_MIN_LEVEL: i32 = -6;
const TERMINAL_ZOOM_MAX_LEVEL: i32 = 12;
// ponytail: mirrors forktty-terminal's byte-per-line budget; expose byte
// scrollback in GhosttyCoreOptions if exact Ghostty scrollback-limit matters.
const GHOSTTY_SCROLLBACK_BYTES_PER_LINE: usize = 2048;
const MAX_GHOSTTY_APPEARANCE_FILE_BYTES: u64 = 1024 * 1024;
const MAX_TERMINAL_SCROLLBACK_LINES: usize = 500_000;

pub(super) fn apply_terminal_appearance(widget: &GhosttyTerminalWidget) {
    let config = config::load_config().unwrap_or_default();
    let gtk_widget = widget.widget();
    let font = terminal_font_description(&gtk_widget, &config);
    gtk_widget.add_css_class("ghostty-terminal");
    gtk_widget.add_css_class("monospace");
    let colors = terminal_colors_for_config(&config);
    let css = format!(
        ".ghostty-terminal {{ font-family: \"{}\"; background: {}; color: {}; }}",
        font.family().unwrap_or_else(|| "monospace".into()),
        colors.background,
        colors.foreground
    );
    TERMINAL_CSS_PROVIDER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let provider = slot.get_or_insert_with(|| {
            let provider = gtk::CssProvider::new();
            if let Some(display) = gtk::gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
            provider
        });
        provider.load_from_data(&css);
    });
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct GhosttyTerminalAppearance {
    pub(super) font_family: Option<String>,
    pub(super) font_family_bold: Option<String>,
    pub(super) font_family_italic: Option<String>,
    pub(super) font_family_bold_italic: Option<String>,
    pub(super) font_size_pt: Option<f64>,
    pub(super) scrollback_limit_bytes: Option<usize>,
    pub(super) colors: TerminalColors,
    bold_color_explicit: bool,
}

pub(super) struct TerminalFontVariants {
    pub(super) bold: Option<gtk::pango::FontDescription>,
    pub(super) italic: Option<gtk::pango::FontDescription>,
    pub(super) bold_italic: Option<gtk::pango::FontDescription>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TerminalColors {
    pub(super) background: String,
    pub(super) foreground: String,
    pub(super) bold: String,
    pub(super) bold_is_bright: bool,
    pub(super) cursor: String,
    pub(super) cursor_foreground: String,
    pub(super) highlight: String,
    pub(super) highlight_foreground: String,
    pub(super) ansi: [String; 16],
}

pub(super) fn terminal_colors_for_config(config: &config::AppConfig) -> TerminalColors {
    ghostty_terminal_appearance_for_config(config).colors
}

pub(super) fn terminal_scrollback_lines_for_config(config: &config::AppConfig) -> usize {
    let appearance = ghostty_terminal_appearance_for_config(config);
    terminal_scrollback_lines_for_appearance(config, &appearance)
}

pub(super) fn terminal_scrollback_lines_for_appearance(
    config: &config::AppConfig,
    appearance: &GhosttyTerminalAppearance,
) -> usize {
    appearance
        .scrollback_limit_bytes
        .map(|bytes| {
            bytes
                .div_ceil(GHOSTTY_SCROLLBACK_BYTES_PER_LINE)
                .min(MAX_TERMINAL_SCROLLBACK_LINES)
        })
        .unwrap_or(config.appearance.scrollback_lines as usize)
}

pub(super) fn terminal_font_description(
    _widget: &impl IsA<gtk::Widget>,
    config: &config::AppConfig,
) -> gtk::pango::FontDescription {
    terminal_font_description_for_zoom_level(config, 0)
}

#[cfg(test)]
pub(super) fn default_terminal_font_description(
    config: &config::AppConfig,
) -> gtk::pango::FontDescription {
    let appearance = ghostty_terminal_appearance_for_config(config);
    let mut font = gtk::pango::FontDescription::from_string("monospace");
    if let Some(family) = appearance.font_family {
        font.set_family(&family);
    }
    if let Some(size) = appearance.font_size_pt {
        font.set_size((size * f64::from(gtk::pango::SCALE)).round() as i32);
    }
    font
}

pub(super) fn next_terminal_zoom_level(current: i32, delta: i32) -> i32 {
    current
        .saturating_add(delta)
        .clamp(TERMINAL_ZOOM_MIN_LEVEL, TERMINAL_ZOOM_MAX_LEVEL)
}

pub(super) fn terminal_font_description_for_zoom_level(
    config: &config::AppConfig,
    zoom_level: i32,
) -> gtk::pango::FontDescription {
    let appearance = ghostty_terminal_appearance_for_config(config);
    let mut font = gtk::pango::FontDescription::from_string("monospace");
    if let Some(family) = appearance.font_family {
        font.set_family(&family);
    }
    let zoom_level = next_terminal_zoom_level(zoom_level, 0);
    if zoom_level != 0 {
        let base = appearance
            .font_size_pt
            .unwrap_or(f64::from(TERMINAL_ZOOM_BASE_PT));
        let size = (base + f64::from(zoom_level)).max(1.0);
        font.set_size((size * f64::from(gtk::pango::SCALE)).round() as i32);
    } else if let Some(size) = appearance.font_size_pt {
        font.set_size((size * f64::from(gtk::pango::SCALE)).round() as i32);
    }
    font
}

pub(super) fn terminal_font_variants_for_config(
    config: &config::AppConfig,
    base: &gtk::pango::FontDescription,
) -> TerminalFontVariants {
    let appearance = ghostty_terminal_appearance_for_config(config);
    TerminalFontVariants {
        bold: appearance
            .font_family_bold
            .as_deref()
            .map(|family| styled_terminal_font_description(base, family, true, false)),
        italic: appearance
            .font_family_italic
            .as_deref()
            .map(|family| styled_terminal_font_description(base, family, false, true)),
        bold_italic: appearance
            .font_family_bold_italic
            .as_deref()
            .map(|family| styled_terminal_font_description(base, family, true, true)),
    }
}

fn styled_terminal_font_description(
    base: &gtk::pango::FontDescription,
    family: &str,
    bold: bool,
    italic: bool,
) -> gtk::pango::FontDescription {
    let mut font = base.clone();
    font.set_family(family);
    if bold {
        font.set_weight(gtk::pango::Weight::Bold);
    }
    if italic {
        font.set_style(gtk::pango::Style::Italic);
    }
    font
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GhosttyColorScheme {
    Light,
    Dark,
}

fn ghostty_terminal_appearance_for_config(config: &config::AppConfig) -> GhosttyTerminalAppearance {
    ghostty_terminal_appearance(ghostty_color_scheme_for_config(config))
}

fn ghostty_color_scheme_for_config(config: &config::AppConfig) -> GhosttyColorScheme {
    if config.general.theme_source.eq_ignore_ascii_case("light") {
        GhosttyColorScheme::Light
    } else {
        GhosttyColorScheme::Dark
    }
}

#[cfg(test)]
fn ghostty_terminal_appearance(_color_scheme: GhosttyColorScheme) -> GhosttyTerminalAppearance {
    GhosttyTerminalAppearance::default()
}

#[cfg(not(test))]
fn ghostty_terminal_appearance(color_scheme: GhosttyColorScheme) -> GhosttyTerminalAppearance {
    load_ghostty_terminal_appearance(
        &ghostty_config_paths(),
        &ghostty_theme_search_dirs(),
        color_scheme,
    )
}

#[cfg(not(test))]
fn ghostty_config_paths() -> Vec<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    let Some(base) = base else {
        return Vec::new();
    };
    let ghostty = base.join("ghostty");
    vec![ghostty.join("config"), ghostty.join("config.ghostty")]
}

#[cfg(not(test))]
fn ghostty_theme_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = BTreeSet::new();

    let mut push = |path: PathBuf| {
        let path = expand_home_path(path);
        if seen.insert(path.clone()) {
            dirs.push(path);
        }
    };

    if let Some(resources) = std::env::var_os("GHOSTTY_RESOURCES_DIR") {
        push(PathBuf::from(resources).join("themes"));
    }
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        push(PathBuf::from(data_home).join("ghostty").join("themes"));
    } else if let Some(home) = std::env::var_os("HOME") {
        push(
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("ghostty")
                .join("themes"),
        );
    }
    if let Some(data_dirs) = std::env::var_os("XDG_DATA_DIRS") {
        for dir in std::env::split_paths(&data_dirs) {
            push(dir.join("ghostty").join("themes"));
        }
    }
    push(PathBuf::from("/usr/local/share/ghostty/themes"));
    push(PathBuf::from("/usr/share/ghostty/themes"));
    if let Some(home) = std::env::var_os("HOME") {
        push(
            PathBuf::from(home)
                .join(".config")
                .join("ghostty")
                .join("themes"),
        );
    }

    dirs
}

#[cfg(test)]
pub(super) fn ghostty_terminal_appearance_from_text(text: &str) -> GhosttyTerminalAppearance {
    let mut appearance = GhosttyTerminalAppearance::default();
    apply_ghostty_terminal_appearance_text(&mut appearance, text);
    appearance
}

#[cfg(test)]
pub(super) fn ghostty_terminal_appearance_from_paths_for_test(
    config_paths: &[PathBuf],
    theme_dirs: &[PathBuf],
    color_scheme: GhosttyColorScheme,
) -> GhosttyTerminalAppearance {
    load_ghostty_terminal_appearance(config_paths, theme_dirs, color_scheme)
}

fn load_ghostty_terminal_appearance(
    config_paths: &[PathBuf],
    theme_dirs: &[PathBuf],
    color_scheme: GhosttyColorScheme,
) -> GhosttyTerminalAppearance {
    let mut appearance = GhosttyTerminalAppearance::default();
    let mut recursive_config_paths = Vec::new();
    let mut loaded_recursive_config_paths = BTreeSet::new();

    for path in config_paths {
        load_ghostty_config_file(
            &mut appearance,
            path,
            theme_dirs,
            color_scheme,
            &mut recursive_config_paths,
            &mut loaded_recursive_config_paths,
            false,
        );
    }

    while !recursive_config_paths.is_empty() {
        let path = recursive_config_paths.remove(0);
        load_ghostty_config_file(
            &mut appearance,
            &path,
            theme_dirs,
            color_scheme,
            &mut recursive_config_paths,
            &mut loaded_recursive_config_paths,
            true,
        );
    }

    appearance
}

fn load_ghostty_config_file(
    appearance: &mut GhosttyTerminalAppearance,
    path: &Path,
    theme_dirs: &[PathBuf],
    color_scheme: GhosttyColorScheme,
    recursive_config_paths: &mut Vec<PathBuf>,
    loaded_recursive_config_paths: &mut BTreeSet<PathBuf>,
    mark_loaded_path: bool,
) {
    let path = expand_home_path(path.to_path_buf());
    if mark_loaded_path && !loaded_recursive_config_paths.insert(path.clone()) {
        return;
    }
    let Some(text) = read_ghostty_appearance_file(&path) else {
        return;
    };

    apply_ghostty_terminal_appearance_text_with_themes(appearance, &text, theme_dirs, color_scheme);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    collect_ghostty_config_file_directives(&text, parent, recursive_config_paths);
}

fn apply_ghostty_terminal_appearance_text(appearance: &mut GhosttyTerminalAppearance, text: &str) {
    for entry in text.lines().filter_map(parse_ghostty_config_entry) {
        appearance.apply(&entry.key, entry.value);
    }
}

fn apply_ghostty_terminal_appearance_text_with_themes(
    appearance: &mut GhosttyTerminalAppearance,
    text: &str,
    theme_dirs: &[PathBuf],
    color_scheme: GhosttyColorScheme,
) {
    for entry in text.lines().filter_map(parse_ghostty_config_entry) {
        if entry.key == "theme" {
            let Some(value) = entry.value else {
                continue;
            };
            load_ghostty_theme(appearance, &value, theme_dirs, color_scheme);
        } else {
            appearance.apply(&entry.key, entry.value);
        }
    }
}

fn collect_ghostty_config_file_directives(
    text: &str,
    parent_dir: &Path,
    recursive_config_paths: &mut Vec<PathBuf>,
) {
    for entry in text.lines().filter_map(parse_ghostty_config_entry) {
        if entry.key != "config-file" {
            continue;
        }
        let Some(value) = entry.value else {
            continue;
        };
        apply_ghostty_config_file_directive(
            value,
            entry.value_was_quoted,
            parent_dir,
            recursive_config_paths,
        );
    }
}

fn apply_ghostty_config_file_directive(
    value: String,
    value_was_quoted: bool,
    parent_dir: &Path,
    recursive_config_paths: &mut Vec<PathBuf>,
) {
    let mut include_path = value;
    if include_path.is_empty() {
        recursive_config_paths.clear();
        return;
    }
    if !value_was_quoted && include_path.starts_with('?') {
        include_path.remove(0);
        include_path = unquote_ghostty_value(include_path.trim());
    }
    if include_path.is_empty() {
        return;
    }

    let expanded = expand_home_path(PathBuf::from(include_path));
    if expanded.is_absolute() {
        recursive_config_paths.push(expanded);
    } else {
        recursive_config_paths.push(parent_dir.join(expanded));
    }
}

impl Default for GhosttyTerminalAppearance {
    fn default() -> Self {
        Self {
            font_family: None,
            font_family_bold: None,
            font_family_italic: None,
            font_family_bold_italic: None,
            font_size_pt: None,
            scrollback_limit_bytes: None,
            colors: TerminalColors::forktty_dark(),
            bold_color_explicit: false,
        }
    }
}

impl GhosttyTerminalAppearance {
    fn apply(&mut self, key: &str, value: Option<String>) {
        let value = value.unwrap_or_default();
        match key {
            "font-family" => self.apply_font_family(value),
            "font-family-bold" => apply_font_family_value(&mut self.font_family_bold, value),
            "font-family-italic" => apply_font_family_value(&mut self.font_family_italic, value),
            "font-family-bold-italic" => {
                apply_font_family_value(&mut self.font_family_bold_italic, value);
            }
            "font-size" => {
                self.font_size_pt = value
                    .parse::<f64>()
                    .ok()
                    .filter(|size| size.is_finite() && *size > 0.0)
            }
            "scrollback-limit" => {
                self.scrollback_limit_bytes = parse_ghostty_integer_literal(&value)
            }
            "background" => {
                set_color(&mut self.colors.background, &value);
            }
            "foreground" => {
                set_color(&mut self.colors.foreground, &value);
                if !self.bold_color_explicit {
                    set_color(&mut self.colors.bold, &value);
                }
            }
            "bold-color" => {
                if value == "bright" {
                    self.colors.bold_is_bright = true;
                    self.bold_color_explicit = true;
                } else if set_color(&mut self.colors.bold, &value) {
                    self.colors.bold_is_bright = false;
                    self.bold_color_explicit = true;
                }
            }
            "bold-is-bright" => {
                if parse_ghostty_bool(&value) {
                    self.colors.bold_is_bright = true;
                    self.bold_color_explicit = true;
                }
            }
            "cursor-color" => set_terminal_color(
                &mut self.colors.cursor,
                &value,
                &self.colors.foreground,
                &self.colors.background,
            ),
            "cursor-text" => set_terminal_color(
                &mut self.colors.cursor_foreground,
                &value,
                &self.colors.foreground,
                &self.colors.background,
            ),
            "cursor-invert-fg-bg" => {
                if parse_ghostty_bool(&value) {
                    self.colors.cursor = self.colors.foreground.clone();
                    self.colors.cursor_foreground = self.colors.background.clone();
                }
            }
            "selection-background" => set_terminal_color(
                &mut self.colors.highlight,
                &value,
                &self.colors.foreground,
                &self.colors.background,
            ),
            "selection-foreground" => set_terminal_color(
                &mut self.colors.highlight_foreground,
                &value,
                &self.colors.foreground,
                &self.colors.background,
            ),
            "selection-invert-fg-bg" => {
                if parse_ghostty_bool(&value) {
                    self.colors.highlight = self.colors.foreground.clone();
                    self.colors.highlight_foreground = self.colors.background.clone();
                }
            }
            "palette" => {
                if let Some((index, color)) = value.split_once('=').and_then(|(index, color)| {
                    Some((
                        parse_palette_index(index.trim())?,
                        normalize_ghostty_color(color.trim())?,
                    ))
                }) {
                    if index < self.colors.ansi.len() {
                        self.colors.ansi[index] = color;
                    }
                }
            }
            _ => {}
        }
    }

    fn apply_font_family(&mut self, value: String) {
        apply_font_family_value(&mut self.font_family, value);
    }
}

fn apply_font_family_value(target: &mut Option<String>, value: String) {
    if value.is_empty() {
        *target = None;
        return;
    }
    match target {
        Some(existing) => {
            existing.push_str(", ");
            existing.push_str(&value);
        }
        None => *target = Some(value),
    }
}

impl TerminalColors {
    pub(super) fn forktty_dark() -> Self {
        let ansi = [
            "#2a2a2a", "#d36b6b", "#7ca982", "#c8a75d", "#6f8fbf", "#a083b8", "#6da7b3", "#d7d7d7",
            "#5f5f5f", "#e07a7a", "#8bb892", "#d7b86f", "#83a4d4", "#b493c8", "#80b7c1", "#f0f0f0",
        ];
        Self {
            background: "#181818".to_string(),
            foreground: "#d7d7d7".to_string(),
            bold: "#f0f0f0".to_string(),
            bold_is_bright: false,
            cursor: "#d7d7d7".to_string(),
            cursor_foreground: "#181818".to_string(),
            highlight: "#3a2a1f".to_string(),
            highlight_foreground: "#eeeeee".to_string(),
            ansi: ansi.map(str::to_string),
        }
    }
}

fn set_color(target: &mut String, value: &str) -> bool {
    if let Some(color) = normalize_ghostty_color(value) {
        *target = color;
        true
    } else {
        false
    }
}

fn set_terminal_color(target: &mut String, value: &str, foreground: &str, background: &str) {
    match value {
        "cell-foreground" => *target = foreground.to_string(),
        "cell-background" => *target = background.to_string(),
        _ => {
            set_color(target, value);
        }
    }
}

fn parse_ghostty_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "1" | "true" | "t" | "yes" | "y" | "on"
    )
}

fn load_ghostty_theme(
    appearance: &mut GhosttyTerminalAppearance,
    raw_name: &str,
    theme_dirs: &[PathBuf],
    color_scheme: GhosttyColorScheme,
) {
    let resolved = resolve_ghostty_theme_name(raw_name, color_scheme);
    if resolved.is_empty() {
        return;
    }

    let expanded = expand_home_path(PathBuf::from(&resolved));
    if expanded.is_absolute() {
        if let Some(text) = read_ghostty_appearance_file(&expanded) {
            apply_ghostty_terminal_appearance_text(appearance, &text);
        }
        return;
    }

    for candidate in ghostty_theme_name_candidates(&resolved) {
        for dir in theme_dirs {
            let path = dir.join(&candidate);
            if let Some(text) = read_ghostty_appearance_file(&path) {
                apply_ghostty_terminal_appearance_text(appearance, &text);
                return;
            }
        }
    }
}

fn read_ghostty_appearance_file(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_GHOSTTY_APPEARANCE_FILE_BYTES
    {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn resolve_ghostty_theme_name(raw_name: &str, color_scheme: GhosttyColorScheme) -> String {
    let mut fallback_theme = None;
    let mut light_theme = None;
    let mut dark_theme = None;

    for entry in raw_name
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let Some((key, value)) = entry.split_once(':') else {
            fallback_theme.get_or_insert_with(|| entry.to_string());
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim().to_ascii_lowercase().as_str() {
            "light" => {
                light_theme.get_or_insert_with(|| value.to_string());
            }
            "dark" => {
                dark_theme.get_or_insert_with(|| value.to_string());
            }
            _ => {
                fallback_theme.get_or_insert_with(|| value.to_string());
            }
        }
    }

    match color_scheme {
        GhosttyColorScheme::Light => light_theme.as_ref(),
        GhosttyColorScheme::Dark => dark_theme.as_ref(),
    }
    .or(fallback_theme.as_ref())
    .or(dark_theme.as_ref())
    .or(light_theme.as_ref())
    .cloned()
    .unwrap_or_else(|| raw_name.trim().to_string())
}

fn ghostty_theme_name_candidates(raw_name: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut queue = vec![raw_name.trim().to_string()];

    while let Some(name) = queue.pop() {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        push_ghostty_theme_candidate(&mut candidates, name);

        let lower = name.to_ascii_lowercase();
        if lower.starts_with("builtin ") {
            let stripped = name["builtin ".len()..].trim();
            push_ghostty_theme_candidate(&mut candidates, stripped);
            queue.push(stripped.to_string());
        }
        if let Some(stripped) = name.strip_suffix("(builtin)") {
            let stripped = stripped.trim();
            push_ghostty_theme_candidate(&mut candidates, stripped);
            queue.push(stripped.to_string());
        }
    }

    candidates
}

fn push_ghostty_theme_candidate(candidates: &mut Vec<String>, candidate: &str) {
    let candidate = candidate.trim();
    if candidate.is_empty() {
        return;
    }
    push_unique_case_sensitive(candidates, candidate);
    match candidate.to_ascii_lowercase().as_str() {
        "solarized light" => push_unique_case_sensitive(candidates, "iTerm2 Solarized Light"),
        "iterm2 solarized light" => push_unique_case_sensitive(candidates, "Solarized Light"),
        "solarized dark" => push_unique_case_sensitive(candidates, "iTerm2 Solarized Dark"),
        "iterm2 solarized dark" => push_unique_case_sensitive(candidates, "Solarized Dark"),
        _ => {}
    }
}

fn push_unique_case_sensitive(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

struct GhosttyConfigEntry {
    key: String,
    value: Option<String>,
    value_was_quoted: bool,
}

fn parse_ghostty_config_entry(line: &str) -> Option<GhosttyConfigEntry> {
    let mut line = line.trim();
    if let Some(stripped) = line.strip_prefix('\u{feff}') {
        line = stripped.trim();
    }
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    let Some((key, value)) = line.split_once('=') else {
        return Some(GhosttyConfigEntry {
            key: line.trim().to_string(),
            value: None,
            value_was_quoted: false,
        });
    };

    let key = key.trim().to_string();
    let value = value.trim();
    let value_was_quoted = value.len() >= 2 && value.starts_with('"') && value.ends_with('"');
    Some(GhosttyConfigEntry {
        key,
        value: Some(unquote_ghostty_value(value)),
        value_was_quoted,
    })
}

fn normalize_ghostty_color(value: &str) -> Option<String> {
    normalize_hex_color(value).or_else(|| normalize_gdk_color(value))
}

fn normalize_hex_color(value: &str) -> Option<String> {
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() == 3 && value.chars().all(|ch| ch.is_ascii_hexdigit()) {
        let mut expanded = String::from("#");
        for ch in value.chars() {
            expanded.push(ch.to_ascii_lowercase());
            expanded.push(ch.to_ascii_lowercase());
        }
        return Some(expanded);
    }
    (value.len() == 6 && value.chars().all(|ch| ch.is_ascii_hexdigit()))
        .then(|| format!("#{}", value.to_ascii_lowercase()))
}

fn normalize_gdk_color(value: &str) -> Option<String> {
    let rgba = gtk::gdk::RGBA::parse(value).ok()?;
    Some(format!(
        "#{:02x}{:02x}{:02x}",
        color_component_to_u8(rgba.red()),
        color_component_to_u8(rgba.green()),
        color_component_to_u8(rgba.blue())
    ))
}

fn color_component_to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn parse_palette_index(value: &str) -> Option<usize> {
    if let Some(value) = value.strip_prefix("0x") {
        usize::from_str_radix(value, 16).ok()
    } else if let Some(value) = value.strip_prefix("0o") {
        usize::from_str_radix(value, 8).ok()
    } else if let Some(value) = value.strip_prefix("0b") {
        usize::from_str_radix(value, 2).ok()
    } else {
        value.parse().ok()
    }
}

fn parse_ghostty_integer_literal(value: &str) -> Option<usize> {
    value.replace('_', "").parse().ok()
}

fn unquote_ghostty_value(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
        .to_string()
}

fn expand_home_path(path: PathBuf) -> PathBuf {
    let Some(raw) = path.to_str() else {
        return path;
    };
    if raw == "~" {
        return std::env::var_os("HOME").map(PathBuf::from).unwrap_or(path);
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path
}
