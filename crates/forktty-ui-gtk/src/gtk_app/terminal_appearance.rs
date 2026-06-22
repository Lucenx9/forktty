use super::*;
use forktty_terminal::ghostty::core::TerminalCursorStyle;
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
const MAX_GHOSTTY_IMAGE_STORAGE_LIMIT_BYTES: u64 = u32::MAX as u64;
const DEFAULT_UNFOCUSED_SPLIT_OPACITY: f64 = 0.92;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct MouseScrollMultipliers {
    pub(super) precision: f64,
    pub(super) discrete: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum GhosttyMetricAdjustment {
    Pixels(i32),
    Percent(f64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GhosttyFontStyleChoice {
    Named(String),
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalRightClickAction {
    ContextMenu,
    Paste,
    Copy,
    CopyOrPaste,
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalCopyOnSelect {
    Disabled,
    Selection,
    Clipboard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClipboardCodepointMapping {
    pub(super) start: char,
    pub(super) end: char,
    pub(super) replacement: String,
}

impl ClipboardCodepointMapping {
    fn contains(&self, ch: char) -> bool {
        self.start <= ch && ch <= self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GhosttyScrollToBottom {
    pub(super) keystroke: bool,
    pub(super) output: bool,
}

impl Default for MouseScrollMultipliers {
    fn default() -> Self {
        Self {
            precision: 1.0,
            discrete: 3.0,
        }
    }
}

impl Default for GhosttyScrollToBottom {
    fn default() -> Self {
        Self {
            keystroke: true,
            output: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum GhosttyMouseShiftCapture {
    #[default]
    False,
    True,
    Never,
    Always,
}

impl GhosttyMouseShiftCapture {
    pub(super) fn capture(self, runtime_override: Option<bool>) -> bool {
        match self {
            Self::False => runtime_override.unwrap_or(false),
            Self::True => runtime_override.unwrap_or(true),
            Self::Never => false,
            Self::Always => true,
        }
    }
}

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
    pub(super) font_style: Option<GhosttyFontStyleChoice>,
    pub(super) font_style_bold: Option<GhosttyFontStyleChoice>,
    pub(super) font_style_italic: Option<GhosttyFontStyleChoice>,
    pub(super) font_style_bold_italic: Option<GhosttyFontStyleChoice>,
    pub(super) synthetic_bold: bool,
    pub(super) synthetic_italic: bool,
    pub(super) synthetic_bold_italic: bool,
    pub(super) font_features: Vec<String>,
    pub(super) font_variation: Option<String>,
    pub(super) font_variation_bold: Option<String>,
    pub(super) font_variation_italic: Option<String>,
    pub(super) font_variation_bold_italic: Option<String>,
    pub(super) font_size_pt: Option<f64>,
    pub(super) scrollback_limit_bytes: Option<usize>,
    pub(super) scrollbar: GhosttyScrollbarPolicy,
    pub(super) image_storage_limit_bytes: Option<u64>,
    pub(super) cursor_style: Option<TerminalCursorStyle>,
    pub(super) cursor_style_blink: Option<bool>,
    pub(super) selection_clear_on_typing: Option<bool>,
    pub(super) selection_clear_on_copy: Option<bool>,
    pub(super) selection_word_chars: Option<Vec<char>>,
    pub(super) clipboard_trim_trailing_spaces: bool,
    pub(super) clipboard_codepoint_map: Vec<ClipboardCodepointMapping>,
    pub(super) copy_on_select: TerminalCopyOnSelect,
    pub(super) right_click_action: Option<TerminalRightClickAction>,
    pub(super) scroll_to_bottom: GhosttyScrollToBottom,
    pub(super) cursor_opacity: f64,
    pub(super) faint_opacity: f64,
    pub(super) mouse_scroll_multipliers: MouseScrollMultipliers,
    pub(super) mouse_reporting: Option<bool>,
    pub(super) mouse_shift_capture: GhosttyMouseShiftCapture,
    pub(super) mouse_hide_while_typing: bool,
    pub(super) adjust_cell_width: Option<GhosttyMetricAdjustment>,
    pub(super) adjust_cell_height: Option<GhosttyMetricAdjustment>,
    pub(super) adjust_font_baseline: Option<GhosttyMetricAdjustment>,
    pub(super) adjust_underline_position: Option<GhosttyMetricAdjustment>,
    pub(super) adjust_underline_thickness: Option<GhosttyMetricAdjustment>,
    pub(super) adjust_strikethrough_position: Option<GhosttyMetricAdjustment>,
    pub(super) adjust_strikethrough_thickness: Option<GhosttyMetricAdjustment>,
    pub(super) adjust_overline_position: Option<GhosttyMetricAdjustment>,
    pub(super) adjust_overline_thickness: Option<GhosttyMetricAdjustment>,
    pub(super) adjust_cursor_thickness: Option<GhosttyMetricAdjustment>,
    pub(super) adjust_cursor_height: Option<GhosttyMetricAdjustment>,
    pub(super) unfocused_split_opacity: f64,
    pub(super) unfocused_split_fill: String,
    pub(super) colors: TerminalColors,
    bold_color_explicit: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum GhosttyScrollbarPolicy {
    #[default]
    System,
    Never,
}

pub(super) struct TerminalFontVariants {
    pub(super) font_features: Option<String>,
    pub(super) bold: Option<gtk::pango::FontDescription>,
    pub(super) italic: Option<gtk::pango::FontDescription>,
    pub(super) bold_italic: Option<gtk::pango::FontDescription>,
    pub(super) synthetic_bold: bool,
    pub(super) synthetic_italic: bool,
    pub(super) synthetic_bold_italic: bool,
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

pub(super) fn terminal_mouse_scroll_multipliers_for_config(
    config: &config::AppConfig,
) -> MouseScrollMultipliers {
    ghostty_terminal_appearance_for_config(config).mouse_scroll_multipliers
}

pub(super) fn terminal_mouse_reporting_for_config(config: &config::AppConfig) -> bool {
    let appearance = ghostty_terminal_appearance_for_config(config);
    terminal_mouse_reporting_for_appearance(config, &appearance)
}

pub(super) fn terminal_mouse_reporting_for_appearance(
    _config: &config::AppConfig,
    appearance: &GhosttyTerminalAppearance,
) -> bool {
    appearance.mouse_reporting.unwrap_or(true)
}

pub(super) fn terminal_mouse_shift_capture_for_config(
    config: &config::AppConfig,
) -> GhosttyMouseShiftCapture {
    let appearance = ghostty_terminal_appearance_for_config(config);
    terminal_mouse_shift_capture_for_appearance(config, &appearance)
}

pub(super) fn terminal_mouse_shift_capture_for_appearance(
    _config: &config::AppConfig,
    appearance: &GhosttyTerminalAppearance,
) -> GhosttyMouseShiftCapture {
    appearance.mouse_shift_capture
}

pub(super) fn terminal_mouse_hide_while_typing_for_config(config: &config::AppConfig) -> bool {
    ghostty_terminal_appearance_for_config(config).mouse_hide_while_typing
}

pub(super) fn terminal_kitty_image_storage_limit_for_config(
    config: &config::AppConfig,
) -> Option<u64> {
    ghostty_terminal_appearance_for_config(config).image_storage_limit_bytes
}

pub(super) fn terminal_cursor_style_sequence_for_config(
    config: &config::AppConfig,
) -> Option<Vec<u8>> {
    let appearance = ghostty_terminal_appearance_for_config(config);
    cursor_style_sequence_for_appearance(&appearance)
}

pub(super) fn terminal_selection_clear_on_typing_for_config(config: &config::AppConfig) -> bool {
    let appearance = ghostty_terminal_appearance_for_config(config);
    terminal_selection_clear_on_typing_for_appearance(config, &appearance)
}

pub(super) fn terminal_selection_clear_on_typing_for_appearance(
    _config: &config::AppConfig,
    appearance: &GhosttyTerminalAppearance,
) -> bool {
    appearance.selection_clear_on_typing.unwrap_or(true)
}

pub(super) fn terminal_selection_clear_on_copy_for_config(config: &config::AppConfig) -> bool {
    let appearance = ghostty_terminal_appearance_for_config(config);
    terminal_selection_clear_on_copy_for_appearance(config, &appearance)
}

pub(super) fn terminal_selection_clear_on_copy_for_appearance(
    _config: &config::AppConfig,
    appearance: &GhosttyTerminalAppearance,
) -> bool {
    appearance.selection_clear_on_copy.unwrap_or(false)
}

pub(super) fn terminal_clipboard_trim_trailing_spaces_for_config(
    config: &config::AppConfig,
) -> bool {
    ghostty_terminal_appearance_for_config(config).clipboard_trim_trailing_spaces
}

pub(super) fn terminal_clipboard_codepoint_map_for_config(
    config: &config::AppConfig,
) -> Vec<ClipboardCodepointMapping> {
    let appearance = ghostty_terminal_appearance_for_config(config);
    terminal_clipboard_codepoint_map_for_appearance(config, &appearance)
}

pub(super) fn terminal_clipboard_codepoint_map_for_appearance(
    _config: &config::AppConfig,
    appearance: &GhosttyTerminalAppearance,
) -> Vec<ClipboardCodepointMapping> {
    appearance.clipboard_codepoint_map.clone()
}

pub(super) fn apply_clipboard_codepoint_map(
    text: &str,
    map: &[ClipboardCodepointMapping],
) -> String {
    if map.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if let Some(mapping) = map.iter().rev().find(|mapping| mapping.contains(ch)) {
            out.push_str(&mapping.replacement);
        } else {
            out.push(ch);
        }
    }
    out
}

pub(super) fn terminal_copy_on_select_for_config(
    config: &config::AppConfig,
) -> TerminalCopyOnSelect {
    let appearance = ghostty_terminal_appearance_for_config(config);
    terminal_copy_on_select_for_appearance(config, &appearance)
}

pub(super) fn terminal_copy_on_select_for_appearance(
    _config: &config::AppConfig,
    appearance: &GhosttyTerminalAppearance,
) -> TerminalCopyOnSelect {
    appearance.copy_on_select
}

pub(super) fn terminal_selection_word_chars_for_config(
    config: &config::AppConfig,
) -> Option<Vec<char>> {
    let appearance = ghostty_terminal_appearance_for_config(config);
    terminal_selection_word_chars_for_appearance(config, &appearance)
}

pub(super) fn terminal_selection_word_chars_for_appearance(
    _config: &config::AppConfig,
    appearance: &GhosttyTerminalAppearance,
) -> Option<Vec<char>> {
    appearance.selection_word_chars.clone()
}

pub(super) fn terminal_right_click_action_for_config(
    config: &config::AppConfig,
) -> TerminalRightClickAction {
    let appearance = ghostty_terminal_appearance_for_config(config);
    terminal_right_click_action_for_appearance(config, &appearance)
}

pub(super) fn terminal_right_click_action_for_appearance(
    _config: &config::AppConfig,
    appearance: &GhosttyTerminalAppearance,
) -> TerminalRightClickAction {
    appearance
        .right_click_action
        .unwrap_or(TerminalRightClickAction::ContextMenu)
}

pub(super) fn terminal_scroll_to_bottom_for_config(
    config: &config::AppConfig,
) -> GhosttyScrollToBottom {
    ghostty_terminal_appearance_for_config(config).scroll_to_bottom
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
        apply_font_style_choice(&mut font, &appearance.font_style);
    }
    apply_font_variation(&mut font, appearance.font_variation.as_deref());
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
        apply_font_style_choice(&mut font, &appearance.font_style);
    }
    apply_font_variation(&mut font, appearance.font_variation.as_deref());
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

pub(super) fn terminal_font_variants_for_appearance(
    appearance: &GhosttyTerminalAppearance,
    base: &gtk::pango::FontDescription,
) -> TerminalFontVariants {
    TerminalFontVariants {
        font_features: (!appearance.font_features.is_empty())
            .then(|| appearance.font_features.join(", ")),
        bold: styled_terminal_font_description(
            base,
            appearance.font_family_bold.as_deref(),
            true,
            false,
            appearance.font_variation_bold.as_deref(),
            &appearance.font_style_bold,
        ),
        italic: styled_terminal_font_description(
            base,
            appearance.font_family_italic.as_deref(),
            false,
            true,
            appearance.font_variation_italic.as_deref(),
            &appearance.font_style_italic,
        ),
        bold_italic: styled_terminal_font_description(
            base,
            appearance.font_family_bold_italic.as_deref(),
            true,
            true,
            appearance.font_variation_bold_italic.as_deref(),
            &appearance.font_style_bold_italic,
        ),
        synthetic_bold: appearance.synthetic_bold
            && !matches!(
                appearance.font_style_bold,
                Some(GhosttyFontStyleChoice::Disabled)
            ),
        synthetic_italic: appearance.synthetic_italic
            && !matches!(
                appearance.font_style_italic,
                Some(GhosttyFontStyleChoice::Disabled)
            ),
        synthetic_bold_italic: appearance.synthetic_bold_italic
            && !matches!(
                appearance.font_style_bold_italic,
                Some(GhosttyFontStyleChoice::Disabled)
            ),
    }
}

fn styled_terminal_font_description(
    base: &gtk::pango::FontDescription,
    family: Option<&str>,
    bold: bool,
    italic: bool,
    variation: Option<&str>,
    style: &Option<GhosttyFontStyleChoice>,
) -> Option<gtk::pango::FontDescription> {
    if matches!(style, Some(GhosttyFontStyleChoice::Disabled)) {
        return None;
    }
    if family.is_none() && variation.is_none() {
        return None;
    }
    let mut font = base.clone();
    if let Some(family) = family {
        font.set_family(family);
        apply_font_style_choice(&mut font, style);
    }
    let style_applied = family.is_some() && matches!(style, Some(GhosttyFontStyleChoice::Named(_)));
    if !style_applied && bold {
        font.set_weight(gtk::pango::Weight::Bold);
    }
    if !style_applied && italic {
        font.set_style(gtk::pango::Style::Italic);
    }
    apply_font_variation(&mut font, variation);
    Some(font)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GhosttyColorScheme {
    Light,
    Dark,
}

pub(super) fn ghostty_terminal_appearance_for_config(
    _config: &config::AppConfig,
) -> GhosttyTerminalAppearance {
    ghostty_terminal_appearance(GhosttyColorScheme::Dark)
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
        let colors = TerminalColors::forktty_dark();
        Self {
            font_family: None,
            font_family_bold: None,
            font_family_italic: None,
            font_family_bold_italic: None,
            font_style: None,
            font_style_bold: None,
            font_style_italic: None,
            font_style_bold_italic: None,
            synthetic_bold: true,
            synthetic_italic: true,
            synthetic_bold_italic: true,
            font_features: Vec::new(),
            font_variation: None,
            font_variation_bold: None,
            font_variation_italic: None,
            font_variation_bold_italic: None,
            font_size_pt: None,
            scrollback_limit_bytes: None,
            scrollbar: GhosttyScrollbarPolicy::default(),
            image_storage_limit_bytes: None,
            cursor_style: None,
            cursor_style_blink: None,
            selection_clear_on_typing: None,
            selection_clear_on_copy: None,
            selection_word_chars: None,
            clipboard_trim_trailing_spaces: false,
            clipboard_codepoint_map: Vec::new(),
            copy_on_select: TerminalCopyOnSelect::Selection,
            right_click_action: None,
            scroll_to_bottom: GhosttyScrollToBottom::default(),
            cursor_opacity: 1.0,
            faint_opacity: 0.5,
            mouse_scroll_multipliers: MouseScrollMultipliers::default(),
            mouse_reporting: None,
            mouse_shift_capture: GhosttyMouseShiftCapture::False,
            mouse_hide_while_typing: false,
            adjust_cell_width: None,
            adjust_cell_height: None,
            adjust_font_baseline: None,
            adjust_underline_position: None,
            adjust_underline_thickness: None,
            adjust_strikethrough_position: None,
            adjust_strikethrough_thickness: None,
            adjust_overline_position: None,
            adjust_overline_thickness: None,
            adjust_cursor_thickness: None,
            adjust_cursor_height: None,
            unfocused_split_opacity: DEFAULT_UNFOCUSED_SPLIT_OPACITY,
            unfocused_split_fill: "#000000".to_string(),
            colors,
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
            "font-style" => self.font_style = parse_font_style_choice(&value),
            "font-style-bold" => self.font_style_bold = parse_font_style_choice(&value),
            "font-style-italic" => self.font_style_italic = parse_font_style_choice(&value),
            "font-style-bold-italic" => {
                self.font_style_bold_italic = parse_font_style_choice(&value);
            }
            "font-synthetic-style" => self.apply_font_synthetic_style(&value),
            "font-feature" => self.apply_font_feature(&value),
            "font-variation" => apply_font_variation_value(&mut self.font_variation, &value),
            "font-variation-bold" => {
                apply_font_variation_value(&mut self.font_variation_bold, &value);
            }
            "font-variation-italic" => {
                apply_font_variation_value(&mut self.font_variation_italic, &value);
            }
            "font-variation-bold-italic" => {
                apply_font_variation_value(&mut self.font_variation_bold_italic, &value);
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
            "scrollbar" => self.apply_scrollbar_policy(&value),
            "image-storage-limit" => {
                self.image_storage_limit_bytes = parse_ghostty_byte_limit(&value)
            }
            "cursor-style" => self.apply_cursor_style(&value),
            "cursor-style-blink" => self.apply_cursor_style_blink(&value),
            "selection-clear-on-typing" => {
                self.selection_clear_on_typing = parse_ghostty_optional_bool(&value)
            }
            "selection-clear-on-copy" => {
                self.selection_clear_on_copy = parse_ghostty_optional_bool(&value)
            }
            "selection-word-chars" => self.apply_selection_word_chars(value),
            "clipboard-trim-trailing-spaces" => {
                self.clipboard_trim_trailing_spaces = parse_ghostty_bool(&value);
            }
            "clipboard-codepoint-map" => self.apply_clipboard_codepoint_map(&value),
            "copy-on-select" => self.apply_copy_on_select(&value),
            "right-click-action" => self.apply_right_click_action(&value),
            "scroll-to-bottom" => self.apply_scroll_to_bottom(&value),
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
            "cursor-opacity" => {
                if let Some(opacity) = value.parse::<f64>().ok().filter(|value| value.is_finite()) {
                    self.cursor_opacity = opacity.clamp(0.0, 1.0);
                }
            }
            "faint-opacity" => {
                if let Some(opacity) = value.parse::<f64>().ok().filter(|value| value.is_finite()) {
                    self.faint_opacity = opacity.clamp(0.0, 1.0);
                }
            }
            "adjust-cell-width" => self.adjust_cell_width = parse_ghostty_metric_adjustment(&value),
            "adjust-cell-height" => {
                self.adjust_cell_height = parse_ghostty_metric_adjustment(&value);
            }
            "adjust-font-baseline" => {
                self.adjust_font_baseline = parse_ghostty_metric_adjustment(&value);
            }
            "adjust-underline-position" => {
                self.adjust_underline_position = parse_ghostty_metric_adjustment(&value);
            }
            "adjust-underline-thickness" => {
                self.adjust_underline_thickness = parse_ghostty_metric_adjustment(&value);
            }
            "adjust-strikethrough-position" => {
                self.adjust_strikethrough_position = parse_ghostty_metric_adjustment(&value);
            }
            "adjust-strikethrough-thickness" => {
                self.adjust_strikethrough_thickness = parse_ghostty_metric_adjustment(&value);
            }
            "adjust-overline-position" => {
                self.adjust_overline_position = parse_ghostty_metric_adjustment(&value);
            }
            "adjust-overline-thickness" => {
                self.adjust_overline_thickness = parse_ghostty_metric_adjustment(&value);
            }
            "adjust-cursor-thickness" => {
                self.adjust_cursor_thickness = parse_ghostty_metric_adjustment(&value);
            }
            "adjust-cursor-height" => {
                self.adjust_cursor_height = parse_ghostty_metric_adjustment(&value);
            }
            "mouse-reporting" => self.mouse_reporting = parse_ghostty_optional_bool(&value),
            "mouse-shift-capture" => self.apply_mouse_shift_capture(&value),
            "mouse-hide-while-typing" => self.mouse_hide_while_typing = parse_ghostty_bool(&value),
            "mouse-scroll-multiplier" => self.apply_mouse_scroll_multiplier(&value),
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
            "unfocused-split-opacity" => {
                if let Some(opacity) = value.parse::<f64>().ok().filter(|value| value.is_finite()) {
                    self.unfocused_split_opacity = opacity.clamp(0.15, 1.0);
                }
            }
            "unfocused-split-fill" => self.apply_unfocused_split_fill(&value),
            _ => {}
        }
    }

    fn apply_cursor_style(&mut self, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            self.cursor_style = None;
            return;
        }
        self.cursor_style = match value {
            "block" => Some(TerminalCursorStyle::Block),
            "bar" => Some(TerminalCursorStyle::Bar),
            "underline" => Some(TerminalCursorStyle::Underline),
            "block_hollow" => Some(TerminalCursorStyle::BlockHollow),
            _ => self.cursor_style,
        };
    }

    fn apply_cursor_style_blink(&mut self, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            self.cursor_style_blink = None;
            return;
        }
        self.cursor_style_blink = parse_ghostty_optional_bool(value).or(self.cursor_style_blink);
    }

    fn apply_scrollbar_policy(&mut self, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            self.scrollbar = GhosttyScrollbarPolicy::default();
            return;
        }
        self.scrollbar = match value {
            "system" => GhosttyScrollbarPolicy::System,
            "never" => GhosttyScrollbarPolicy::Never,
            _ => self.scrollbar,
        };
    }

    fn apply_right_click_action(&mut self, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            self.right_click_action = None;
            return;
        }
        self.right_click_action = match value {
            "context-menu" => Some(TerminalRightClickAction::ContextMenu),
            "paste" => Some(TerminalRightClickAction::Paste),
            "copy" => Some(TerminalRightClickAction::Copy),
            "copy-or-paste" => Some(TerminalRightClickAction::CopyOrPaste),
            "ignore" => Some(TerminalRightClickAction::Ignore),
            _ => self.right_click_action,
        };
    }

    fn apply_copy_on_select(&mut self, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            self.copy_on_select = TerminalCopyOnSelect::Selection;
            return;
        }
        self.copy_on_select = match value {
            "clipboard" => TerminalCopyOnSelect::Clipboard,
            _ => match parse_ghostty_optional_bool(value) {
                Some(true) => TerminalCopyOnSelect::Selection,
                Some(false) => TerminalCopyOnSelect::Disabled,
                None => self.copy_on_select,
            },
        };
    }

    fn apply_selection_word_chars(&mut self, value: String) {
        self.selection_word_chars = if value.is_empty() {
            None
        } else {
            Some(value.chars().collect())
        };
    }

    fn apply_clipboard_codepoint_map(&mut self, value: &str) {
        if value.trim().is_empty() {
            self.clipboard_codepoint_map.clear();
            return;
        }
        if let Some(mappings) = parse_clipboard_codepoint_map(value) {
            self.clipboard_codepoint_map.extend(mappings);
        }
    }

    fn apply_mouse_shift_capture(&mut self, value: &str) {
        self.mouse_shift_capture = match value.trim() {
            "always" => GhosttyMouseShiftCapture::Always,
            "never" => GhosttyMouseShiftCapture::Never,
            "" => GhosttyMouseShiftCapture::False,
            value => match parse_ghostty_optional_bool(value) {
                Some(true) => GhosttyMouseShiftCapture::True,
                Some(false) => GhosttyMouseShiftCapture::False,
                None => self.mouse_shift_capture,
            },
        };
    }

    fn apply_scroll_to_bottom(&mut self, value: &str) {
        if value.trim().is_empty() {
            self.scroll_to_bottom = GhosttyScrollToBottom::default();
            return;
        }
        let mut scroll_to_bottom = GhosttyScrollToBottom::default();
        for part in value
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            match part {
                "keystroke" => scroll_to_bottom.keystroke = true,
                "no-keystroke" => scroll_to_bottom.keystroke = false,
                "output" => scroll_to_bottom.output = true,
                "no-output" => scroll_to_bottom.output = false,
                _ => {}
            }
        }
        self.scroll_to_bottom = scroll_to_bottom;
    }

    fn apply_font_family(&mut self, value: String) {
        apply_font_family_value(&mut self.font_family, value);
    }

    fn apply_font_feature(&mut self, value: &str) {
        if value.trim().is_empty() {
            self.font_features.clear();
            return;
        }
        self.font_features
            .extend(value.split(',').filter_map(parse_font_feature));
    }

    fn apply_font_synthetic_style(&mut self, value: &str) {
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("true") {
            self.synthetic_bold = true;
            self.synthetic_italic = true;
            self.synthetic_bold_italic = true;
            return;
        }
        if value.eq_ignore_ascii_case("false") {
            self.synthetic_bold = false;
            self.synthetic_italic = false;
            self.synthetic_bold_italic = false;
            return;
        }
        for item in value.split(',').map(str::trim) {
            match item {
                "bold" => self.synthetic_bold = true,
                "italic" => self.synthetic_italic = true,
                "bold-italic" => self.synthetic_bold_italic = true,
                "no-bold" => self.synthetic_bold = false,
                "no-italic" => self.synthetic_italic = false,
                "no-bold-italic" => self.synthetic_bold_italic = false,
                _ => {}
            }
        }
    }

    fn apply_unfocused_split_fill(&mut self, value: &str) {
        set_color(&mut self.unfocused_split_fill, value);
    }

    fn apply_mouse_scroll_multiplier(&mut self, value: &str) {
        for part in value
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            if let Some(value) = part.strip_prefix("precision:") {
                if let Some(multiplier) = parse_mouse_scroll_multiplier(value) {
                    self.mouse_scroll_multipliers.precision = multiplier;
                }
            } else if let Some(value) = part.strip_prefix("discrete:") {
                if let Some(multiplier) = parse_mouse_scroll_multiplier(value) {
                    self.mouse_scroll_multipliers.discrete = multiplier;
                }
            } else if let Some(multiplier) = parse_mouse_scroll_multiplier(part) {
                self.mouse_scroll_multipliers = MouseScrollMultipliers {
                    precision: multiplier,
                    discrete: multiplier,
                };
            }
        }
    }
}

fn parse_mouse_scroll_multiplier(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.01, 10_000.0))
}

fn parse_ghostty_metric_adjustment(value: &str) -> Option<GhosttyMetricAdjustment> {
    let value = value.trim();
    if let Some(percent) = value.strip_suffix('%') {
        return percent
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(GhosttyMetricAdjustment::Percent);
    }
    value
        .parse::<i32>()
        .ok()
        .map(GhosttyMetricAdjustment::Pixels)
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

fn apply_font_style_choice(
    font: &mut gtk::pango::FontDescription,
    style: &Option<GhosttyFontStyleChoice>,
) {
    let Some(GhosttyFontStyleChoice::Named(style)) = style else {
        return;
    };
    let Some(family) = font.family().map(|family| family.to_string()) else {
        return;
    };
    let size = font.size();
    let variations = font.variations().map(|value| value.to_string());
    // ponytail: Pango parses common named styles; exact Ghostty face lookup belongs with a future Ghostty renderer bridge.
    let mut styled = gtk::pango::FontDescription::from_string(&format!("{family} {style}"));
    if size > 0 {
        styled.set_size(size);
    }
    if let Some(variations) = variations {
        styled.set_variations(Some(&variations));
    }
    *font = styled;
}

fn apply_font_variation(font: &mut gtk::pango::FontDescription, variation: Option<&str>) {
    if let Some(variation) = variation.filter(|value| !value.trim().is_empty()) {
        font.set_variations(Some(variation));
    }
}

fn parse_font_style_choice(value: &str) -> Option<GhosttyFontStyleChoice> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.eq_ignore_ascii_case("false") {
        return Some(GhosttyFontStyleChoice::Disabled);
    }
    if value.eq_ignore_ascii_case("true") {
        return None;
    }
    Some(GhosttyFontStyleChoice::Named(value.to_string()))
}

fn apply_font_variation_value(target: &mut Option<String>, value: &str) {
    if value.trim().is_empty() {
        *target = None;
        return;
    }
    if let Some(value) = parse_font_variation(value) {
        append_csv_value(target, &value);
    }
}

fn parse_font_variation(value: &str) -> Option<String> {
    let (axis, value) = value.trim().split_once('=')?;
    let axis = strip_font_tag_quotes(axis.trim());
    if axis.chars().count() != 4 {
        return None;
    }
    let value = value.trim();
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())?;
    Some(format!("{axis}={value}"))
}

fn parse_font_feature(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Some((feature, setting)) = value.split_once('=') {
        return Some(format!(
            "{}={}",
            clean_font_feature_name(feature),
            setting.trim()
        ));
    }

    let mut parts = value.split_whitespace();
    let feature = parts.next()?;
    let setting = parts.next();
    if parts.next().is_some() {
        return Some(value.to_string());
    }

    match setting {
        Some("on") | Some("true") => Some(format!("{}=1", clean_font_feature_name(feature))),
        Some("off") | Some("false") => Some(format!("{}=0", clean_font_feature_name(feature))),
        Some(number) if number.parse::<i32>().is_ok() => {
            Some(format!("{}={number}", clean_font_feature_name(feature)))
        }
        Some(_) => Some(value.to_string()),
        None => Some(clean_font_feature_name(feature)),
    }
}

fn clean_font_feature_name(value: &str) -> String {
    if let Some(feature) = value.trim().strip_prefix('+') {
        return format!("+{}", strip_font_tag_quotes(feature));
    }
    if let Some(feature) = value.trim().strip_prefix('-') {
        return format!("-{}", strip_font_tag_quotes(feature));
    }
    strip_font_tag_quotes(value)
}

fn strip_font_tag_quotes(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

fn append_csv_value(target: &mut Option<String>, value: &str) {
    match target {
        Some(existing) => {
            existing.push_str(", ");
            existing.push_str(value);
        }
        None => *target = Some(value.to_string()),
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

fn cursor_style_sequence_for_appearance(appearance: &GhosttyTerminalAppearance) -> Option<Vec<u8>> {
    if appearance.cursor_style.is_none() && appearance.cursor_style_blink.is_none() {
        return None;
    }
    let code = match (
        appearance
            .cursor_style
            .unwrap_or(TerminalCursorStyle::Block),
        appearance.cursor_style_blink.unwrap_or(true),
    ) {
        (TerminalCursorStyle::Block, true) => 1,
        (TerminalCursorStyle::Block, false) => 2,
        (TerminalCursorStyle::Underline, true) => 3,
        (TerminalCursorStyle::Underline, false) => 4,
        (TerminalCursorStyle::Bar, true) => 5,
        (TerminalCursorStyle::Bar, false) => 6,
        (TerminalCursorStyle::BlockHollow, _) => return None,
    };
    Some(format!("\x1b[{code} q").into_bytes())
}

fn parse_ghostty_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "1" | "true" | "t" | "yes" | "y" | "on"
    )
}

fn parse_ghostty_optional_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "t" | "yes" | "y" | "on" => Some(true),
        "0" | "false" | "f" | "no" | "n" | "off" => Some(false),
        _ => None,
    }
}

fn parse_clipboard_codepoint_map(value: &str) -> Option<Vec<ClipboardCodepointMapping>> {
    let (ranges, replacement) = value.split_once('=')?;
    let replacement = parse_ghostty_codepoint(replacement.trim())
        .map(|ch| ch.to_string())
        .unwrap_or_else(|| replacement.trim().to_string());
    let mut mappings = Vec::new();
    for range in ranges
        .split(',')
        .map(str::trim)
        .filter(|range| !range.is_empty())
    {
        let (start, end) = parse_ghostty_codepoint_range(range)?;
        mappings.push(ClipboardCodepointMapping {
            start,
            end,
            replacement: replacement.clone(),
        });
    }
    Some(mappings)
}

fn parse_ghostty_codepoint_range(value: &str) -> Option<(char, char)> {
    let (start, end) = value.split_once('-').unwrap_or((value, value));
    let start = parse_ghostty_codepoint(start)?;
    let end = parse_ghostty_codepoint(end)?;
    (start <= end).then_some((start, end))
}

fn parse_ghostty_codepoint(value: &str) -> Option<char> {
    let value = value.trim();
    let hex = value
        .strip_prefix("U+")
        .or_else(|| value.strip_prefix("u+"))?;
    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
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

fn parse_ghostty_byte_limit(value: &str) -> Option<u64> {
    let compact = value
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '_')
        .collect::<String>();
    let lower = compact.to_ascii_lowercase();
    let (digits, factor) = [
        ("gb", 1_000_000_000_u64),
        ("mb", 1_000_000_u64),
        ("kb", 1_000_u64),
        ("b", 1_u64),
    ]
    .into_iter()
    .find_map(|(suffix, factor)| lower.strip_suffix(suffix).map(|digits| (digits, factor)))
    .unwrap_or((lower.as_str(), 1));
    let bytes = digits.parse::<u64>().ok()?.checked_mul(factor)?;
    (bytes <= MAX_GHOSTTY_IMAGE_STORAGE_LIMIT_BYTES).then_some(bytes)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghostty_appearance_reads_unfocused_split_style() {
        let appearance = ghostty_terminal_appearance_from_text(
            r#"
            unfocused-split-opacity = 0.72
            unfocused-split-fill = #102030
            "#,
        );

        assert_eq!(appearance.unfocused_split_opacity, 0.72);
        assert_eq!(appearance.unfocused_split_fill, "#102030");
    }

    #[test]
    fn ghostty_appearance_reads_font_features_and_variations() {
        let appearance = ghostty_terminal_appearance_from_text(
            r#"
            font-feature = -calt, liga=0
            font-feature = ss01 on
            font-variation = wght=450
            font-variation = slnt=-8
            font-variation-bold = wght=700
            font-variation-italic = ital=1
            font-variation-bold-italic = wght=700
            "#,
        );

        assert_eq!(appearance.font_features, vec!["-calt", "liga=0", "ss01=1"]);
        assert_eq!(
            appearance.font_variation.as_deref(),
            Some("wght=450, slnt=-8")
        );
        assert_eq!(appearance.font_variation_bold.as_deref(), Some("wght=700"));
        assert_eq!(appearance.font_variation_italic.as_deref(), Some("ital=1"));
        assert_eq!(
            appearance.font_variation_bold_italic.as_deref(),
            Some("wght=700")
        );

        let mut base = gtk::pango::FontDescription::from_string("monospace 12");
        apply_font_variation(&mut base, appearance.font_variation.as_deref());
        assert_eq!(base.variations().as_deref(), Some("wght=450, slnt=-8"));

        let variants = terminal_font_variants_for_appearance(&appearance, &base);
        assert_eq!(
            variants.font_features.as_deref(),
            Some("-calt, liga=0, ss01=1")
        );
        assert_eq!(
            variants.bold.unwrap().variations().as_deref(),
            Some("wght=700")
        );
        assert_eq!(
            variants.italic.unwrap().variations().as_deref(),
            Some("ital=1")
        );
        assert_eq!(
            variants.bold_italic.unwrap().variations().as_deref(),
            Some("wght=700")
        );

        let reset = ghostty_terminal_appearance_from_text(
            "font-feature = liga\nfont-feature =\nfont-variation = wght=450\nfont-variation =",
        );
        assert!(reset.font_features.is_empty());
        assert_eq!(reset.font_variation, None);
    }

    #[test]
    fn ghostty_appearance_reads_font_styles_and_synthetic_style() {
        let appearance = ghostty_terminal_appearance_from_text(
            r#"
            font-style = Regular
            font-style-bold = false
            font-style-italic = Oblique
            font-style-bold-italic = Bold Oblique
            font-synthetic-style = no-bold,no-bold-italic
            "#,
        );

        assert_eq!(
            appearance.font_style,
            Some(GhosttyFontStyleChoice::Named("Regular".to_string()))
        );
        assert_eq!(
            appearance.font_style_bold,
            Some(GhosttyFontStyleChoice::Disabled)
        );
        assert_eq!(
            appearance.font_style_italic,
            Some(GhosttyFontStyleChoice::Named("Oblique".to_string()))
        );
        assert_eq!(
            appearance.font_style_bold_italic,
            Some(GhosttyFontStyleChoice::Named("Bold Oblique".to_string()))
        );
        assert!(!appearance.synthetic_bold);
        assert!(appearance.synthetic_italic);
        assert!(!appearance.synthetic_bold_italic);

        let variants = terminal_font_variants_for_appearance(
            &appearance,
            &gtk::pango::FontDescription::from_string("monospace 12"),
        );
        assert!(variants.bold.is_none());
        assert!(!variants.synthetic_bold);
        assert!(variants.synthetic_italic);
        assert!(!variants.synthetic_bold_italic);
    }

    #[test]
    fn ghostty_appearance_reads_cursor_opacity() {
        let appearance = ghostty_terminal_appearance_from_text("cursor-opacity = 0.42");
        assert_eq!(appearance.cursor_opacity, 0.42);

        let clamped = ghostty_terminal_appearance_from_text("cursor-opacity = 2");
        assert_eq!(clamped.cursor_opacity, 1.0);

        let clamped = ghostty_terminal_appearance_from_text("cursor-opacity = -1");
        assert_eq!(clamped.cursor_opacity, 0.0);
    }

    #[test]
    fn ghostty_appearance_reads_faint_opacity() {
        let appearance = ghostty_terminal_appearance_from_text("faint-opacity = 0.35");
        assert_eq!(appearance.faint_opacity, 0.35);

        let clamped = ghostty_terminal_appearance_from_text("faint-opacity = 2");
        assert_eq!(clamped.faint_opacity, 1.0);

        let clamped = ghostty_terminal_appearance_from_text("faint-opacity = -1");
        assert_eq!(clamped.faint_opacity, 0.0);
    }

    #[test]
    fn ghostty_appearance_reads_mouse_scroll_multiplier() {
        let appearance = ghostty_terminal_appearance_from_text("mouse-scroll-multiplier = 2");
        assert_eq!(
            appearance.mouse_scroll_multipliers,
            MouseScrollMultipliers {
                precision: 2.0,
                discrete: 2.0
            }
        );

        let appearance = ghostty_terminal_appearance_from_text(
            "mouse-scroll-multiplier = precision:0.25,discrete:4",
        );
        assert_eq!(
            appearance.mouse_scroll_multipliers,
            MouseScrollMultipliers {
                precision: 0.25,
                discrete: 4.0
            }
        );

        let appearance = ghostty_terminal_appearance_from_text(
            "mouse-scroll-multiplier = precision:0,discrete:20000",
        );
        assert_eq!(
            appearance.mouse_scroll_multipliers,
            MouseScrollMultipliers {
                precision: 0.01,
                discrete: 10_000.0
            }
        );
    }

    #[test]
    fn ghostty_appearance_reads_mouse_reporting() {
        let config = config::AppConfig::default();
        let appearance = ghostty_terminal_appearance_from_text("mouse-reporting = false");

        assert!(!terminal_mouse_reporting_for_appearance(
            &config,
            &appearance
        ));

        let reset = ghostty_terminal_appearance_from_text("mouse-reporting =");
        assert!(terminal_mouse_reporting_for_appearance(&config, &reset));
    }

    #[test]
    fn ghostty_appearance_reads_mouse_shift_capture() {
        let config = config::AppConfig::default();
        let appearance = ghostty_terminal_appearance_from_text("mouse-shift-capture = always");

        assert_eq!(
            terminal_mouse_shift_capture_for_appearance(&config, &appearance),
            GhosttyMouseShiftCapture::Always
        );

        let never = ghostty_terminal_appearance_from_text("mouse-shift-capture = never");
        assert_eq!(
            terminal_mouse_shift_capture_for_appearance(&config, &never),
            GhosttyMouseShiftCapture::Never
        );

        let yes = ghostty_terminal_appearance_from_text("mouse-shift-capture = true");
        assert_eq!(
            terminal_mouse_shift_capture_for_appearance(&config, &yes),
            GhosttyMouseShiftCapture::True
        );

        let reset = ghostty_terminal_appearance_from_text("mouse-shift-capture =");
        assert_eq!(
            terminal_mouse_shift_capture_for_appearance(&config, &reset),
            GhosttyMouseShiftCapture::False
        );
        assert!(!GhosttyMouseShiftCapture::True.capture(Some(false)));
        assert!(GhosttyMouseShiftCapture::False.capture(Some(true)));
        assert!(GhosttyMouseShiftCapture::Always.capture(Some(false)));
        assert!(!GhosttyMouseShiftCapture::Never.capture(Some(true)));
    }

    #[test]
    fn ghostty_appearance_reads_mouse_hide_while_typing() {
        let config = config::AppConfig::default();
        let default = ghostty_terminal_appearance_from_text("");
        assert!(!terminal_mouse_hide_while_typing_for_config(&config));
        assert!(!default.mouse_hide_while_typing);

        let enabled = ghostty_terminal_appearance_from_text("mouse-hide-while-typing = true");
        assert!(enabled.mouse_hide_while_typing);

        let disabled = ghostty_terminal_appearance_from_text("mouse-hide-while-typing = false");
        assert!(!disabled.mouse_hide_while_typing);
    }

    #[test]
    fn ghostty_appearance_reads_cell_size_adjustments() {
        let appearance = ghostty_terminal_appearance_from_text(
            r#"
            adjust-cell-width = 2
            adjust-cell-height = 10%
            "#,
        );

        assert_eq!(
            appearance.adjust_cell_width,
            Some(GhosttyMetricAdjustment::Pixels(2))
        );
        assert_eq!(
            appearance.adjust_cell_height,
            Some(GhosttyMetricAdjustment::Percent(10.0))
        );
    }

    #[test]
    fn ghostty_appearance_reads_text_metric_adjustments() {
        let appearance = ghostty_terminal_appearance_from_text(
            r#"
            adjust-font-baseline = 1
            adjust-underline-position = -2
            adjust-underline-thickness = 25%
            adjust-strikethrough-position = 10%
            adjust-strikethrough-thickness = 1
            adjust-overline-position = 3
            adjust-overline-thickness = 50%
            adjust-cursor-thickness = 50%
            adjust-cursor-height = -3
            "#,
        );

        assert_eq!(
            appearance.adjust_font_baseline,
            Some(GhosttyMetricAdjustment::Pixels(1))
        );
        assert_eq!(
            appearance.adjust_underline_position,
            Some(GhosttyMetricAdjustment::Pixels(-2))
        );
        assert_eq!(
            appearance.adjust_underline_thickness,
            Some(GhosttyMetricAdjustment::Percent(25.0))
        );
        assert_eq!(
            appearance.adjust_strikethrough_position,
            Some(GhosttyMetricAdjustment::Percent(10.0))
        );
        assert_eq!(
            appearance.adjust_strikethrough_thickness,
            Some(GhosttyMetricAdjustment::Pixels(1))
        );
        assert_eq!(
            appearance.adjust_overline_position,
            Some(GhosttyMetricAdjustment::Pixels(3))
        );
        assert_eq!(
            appearance.adjust_overline_thickness,
            Some(GhosttyMetricAdjustment::Percent(50.0))
        );
        assert_eq!(
            appearance.adjust_cursor_thickness,
            Some(GhosttyMetricAdjustment::Percent(50.0))
        );
        assert_eq!(
            appearance.adjust_cursor_height,
            Some(GhosttyMetricAdjustment::Pixels(-3))
        );
    }

    #[test]
    fn ghostty_appearance_reads_image_storage_limit() {
        let appearance = ghostty_terminal_appearance_from_text("image-storage-limit = 64MB");
        assert_eq!(appearance.image_storage_limit_bytes, Some(64_000_000));

        let disabled = ghostty_terminal_appearance_from_text("image-storage-limit = 0");
        assert_eq!(disabled.image_storage_limit_bytes, Some(0));
    }

    #[test]
    fn ghostty_appearance_reads_scrollbar_policy() {
        let hidden = ghostty_terminal_appearance_from_text("scrollbar = never");
        assert_eq!(hidden.scrollbar, GhosttyScrollbarPolicy::Never);

        let system = ghostty_terminal_appearance_from_text("scrollbar = system");
        assert_eq!(system.scrollbar, GhosttyScrollbarPolicy::System);

        let reset = ghostty_terminal_appearance_from_text(
            r#"
            scrollbar = never
            scrollbar =
            "#,
        );
        assert_eq!(reset.scrollbar, GhosttyScrollbarPolicy::System);

        let unknown = ghostty_terminal_appearance_from_text(
            r#"
            scrollbar = never
            scrollbar = always
            "#,
        );
        assert_eq!(unknown.scrollbar, GhosttyScrollbarPolicy::Never);
    }

    #[test]
    fn ghostty_appearance_reads_cursor_style_defaults() {
        let appearance = ghostty_terminal_appearance_from_text(
            r#"
            cursor-style = bar
            cursor-style-blink = false
            "#,
        );

        assert_eq!(
            appearance.cursor_style,
            Some(forktty_terminal::ghostty::core::TerminalCursorStyle::Bar)
        );
        assert_eq!(appearance.cursor_style_blink, Some(false));
        assert_eq!(
            cursor_style_sequence_for_appearance(&appearance),
            Some(b"\x1b[6 q".to_vec())
        );

        let reset = ghostty_terminal_appearance_from_text(
            r#"
            cursor-style = underline
            cursor-style-blink = false
            cursor-style =
            cursor-style-blink =
            "#,
        );

        assert_eq!(reset.cursor_style, None);
        assert_eq!(reset.cursor_style_blink, None);
        assert_eq!(cursor_style_sequence_for_appearance(&reset), None);
    }

    #[test]
    fn ghostty_appearance_reads_selection_clear_on_typing() {
        let config = config::AppConfig::default();
        let appearance = ghostty_terminal_appearance_from_text("selection-clear-on-typing = false");

        assert!(!terminal_selection_clear_on_typing_for_appearance(
            &config,
            &appearance
        ));

        let reset = ghostty_terminal_appearance_from_text("selection-clear-on-typing =");
        assert!(terminal_selection_clear_on_typing_for_appearance(
            &config, &reset
        ));
    }

    #[test]
    fn ghostty_appearance_reads_selection_clear_on_copy() {
        let config = config::AppConfig::default();
        let appearance = ghostty_terminal_appearance_from_text("selection-clear-on-copy = true");

        assert!(terminal_selection_clear_on_copy_for_appearance(
            &config,
            &appearance
        ));

        let reset = ghostty_terminal_appearance_from_text("selection-clear-on-copy =");
        assert!(!terminal_selection_clear_on_copy_for_appearance(
            &config, &reset
        ));
    }

    #[test]
    fn ghostty_appearance_reads_clipboard_trim_trailing_spaces() {
        let config = config::AppConfig::default();
        let default = ghostty_terminal_appearance_from_text("");
        assert!(!terminal_clipboard_trim_trailing_spaces_for_config(&config));
        assert!(!default.clipboard_trim_trailing_spaces);

        let enabled =
            ghostty_terminal_appearance_from_text("clipboard-trim-trailing-spaces = true");
        assert!(enabled.clipboard_trim_trailing_spaces);

        let disabled =
            ghostty_terminal_appearance_from_text("clipboard-trim-trailing-spaces = false");
        assert!(!disabled.clipboard_trim_trailing_spaces);
    }

    #[test]
    fn ghostty_appearance_reads_clipboard_codepoint_map() {
        let appearance = ghostty_terminal_appearance_from_text(
            "clipboard-codepoint-map = U+2500=U+002D\n\
             clipboard-codepoint-map = U+2502=|\n\
             clipboard-codepoint-map = U+2500-U+2502=box",
        );

        assert_eq!(
            apply_clipboard_codepoint_map("─│┌", &appearance.clipboard_codepoint_map),
            "boxbox┌"
        );

        let reset = ghostty_terminal_appearance_from_text(
            "clipboard-codepoint-map = U+2500=U+002D\n\
             clipboard-codepoint-map =",
        );
        assert!(terminal_clipboard_codepoint_map_for_appearance(
            &config::AppConfig::default(),
            &reset
        )
        .is_empty());
    }

    #[test]
    fn ghostty_appearance_reads_selection_word_chars() {
        let config = config::AppConfig::default();
        let appearance = ghostty_terminal_appearance_from_text("selection-word-chars = .:");

        assert_eq!(
            terminal_selection_word_chars_for_appearance(&config, &appearance),
            Some(vec!['.', ':'])
        );

        let reset = ghostty_terminal_appearance_from_text("selection-word-chars =");
        assert_eq!(
            terminal_selection_word_chars_for_appearance(&config, &reset),
            None
        );
    }

    #[test]
    fn ghostty_appearance_reads_copy_on_select() {
        let config = config::AppConfig::default();
        let default = ghostty_terminal_appearance_from_text("");
        assert_eq!(
            terminal_copy_on_select_for_appearance(&config, &default),
            TerminalCopyOnSelect::Selection
        );

        let clipboard = ghostty_terminal_appearance_from_text("copy-on-select = clipboard");
        assert_eq!(
            terminal_copy_on_select_for_appearance(&config, &clipboard),
            TerminalCopyOnSelect::Clipboard
        );

        let disabled = ghostty_terminal_appearance_from_text("copy-on-select = false");
        assert_eq!(
            terminal_copy_on_select_for_appearance(&config, &disabled),
            TerminalCopyOnSelect::Disabled
        );

        let reset = ghostty_terminal_appearance_from_text(
            r#"
            copy-on-select = false
            copy-on-select =
            "#,
        );
        assert_eq!(
            terminal_copy_on_select_for_appearance(&config, &reset),
            TerminalCopyOnSelect::Selection
        );
    }

    #[test]
    fn ghostty_appearance_reads_right_click_action() {
        let config = config::AppConfig::default();
        let appearance =
            ghostty_terminal_appearance_from_text("right-click-action = copy-or-paste");

        assert_eq!(
            terminal_right_click_action_for_appearance(&config, &appearance),
            TerminalRightClickAction::CopyOrPaste
        );

        let reset = ghostty_terminal_appearance_from_text("right-click-action =");
        assert_eq!(
            terminal_right_click_action_for_appearance(&config, &reset),
            TerminalRightClickAction::ContextMenu
        );
    }

    #[test]
    fn ghostty_appearance_reads_scroll_to_bottom() {
        let appearance =
            ghostty_terminal_appearance_from_text("scroll-to-bottom = no-keystroke, output");

        assert!(!appearance.scroll_to_bottom.keystroke);
        assert!(appearance.scroll_to_bottom.output);

        let reset = ghostty_terminal_appearance_from_text("scroll-to-bottom =");
        assert!(reset.scroll_to_bottom.keystroke);
        assert!(!reset.scroll_to_bottom.output);
    }
}
