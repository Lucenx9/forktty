use super::*;
use std::cell::RefCell;

thread_local! {
    // A single display-global provider, reused across spawns and settings changes.
    // Adding a fresh provider on every call would accumulate stale rules on the display.
    static TERMINAL_CSS_PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
}

pub(super) fn apply_terminal_appearance(widget: &GhosttyTerminalWidget) {
    let config = config::load_config().unwrap_or_default();
    let gtk_widget = widget.widget();
    let font = terminal_font_description(&gtk_widget, &config);
    gtk_widget.add_css_class("ghostty-terminal");
    gtk_widget.add_css_class("monospace");
    let colors = terminal_colors_for_config(&config);
    let css = format!(
        ".ghostty-terminal {{ font: {}; background: {}; color: {}; }}",
        font, colors.background, colors.foreground
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

pub(super) struct TerminalColors {
    pub(super) background: &'static str,
    pub(super) foreground: &'static str,
    pub(super) bold: &'static str,
    pub(super) cursor: &'static str,
    pub(super) cursor_foreground: &'static str,
    pub(super) highlight: &'static str,
    pub(super) highlight_foreground: &'static str,
    pub(super) ansi: [&'static str; 16],
}

pub(super) fn terminal_colors_for_config(config: &config::AppConfig) -> &'static TerminalColors {
    let theme = config.appearance.terminal_theme.trim().to_ascii_lowercase();
    match theme.as_str() {
        config::TERMINAL_THEME_CATPPUCCIN_MOCHA => &CATPPUCCIN_MOCHA_TERMINAL_COLORS,
        config::TERMINAL_THEME_ROSE_PINE => &ROSE_PINE_TERMINAL_COLORS,
        config::TERMINAL_THEME_TOKYO_NIGHT => &TOKYO_NIGHT_TERMINAL_COLORS,
        config::TERMINAL_THEME_DRACULA => &DRACULA_TERMINAL_COLORS,
        config::TERMINAL_THEME_GRUVBOX_DARK => &GRUVBOX_DARK_TERMINAL_COLORS,
        _ => &FORKTTY_DARK_TERMINAL_COLORS,
    }
}

pub(super) const FORKTTY_DARK_TERMINAL_COLORS: TerminalColors = TerminalColors {
    background: "#181818",
    foreground: "#d7d7d7",
    bold: "#f0f0f0",
    cursor: "#d7d7d7",
    cursor_foreground: "#181818",
    highlight: "#3a2a1f",
    highlight_foreground: "#eeeeee",
    ansi: [
        "#2a2a2a", "#d36b6b", "#7ca982", "#c8a75d", "#6f8fbf", "#a083b8", "#6da7b3", "#d7d7d7",
        "#5f5f5f", "#e07a7a", "#8bb892", "#d7b86f", "#83a4d4", "#b493c8", "#80b7c1", "#f0f0f0",
    ],
};

pub(super) const CATPPUCCIN_MOCHA_TERMINAL_COLORS: TerminalColors = TerminalColors {
    background: "#1e1e2e",
    foreground: "#cdd6f4",
    bold: "#cdd6f4",
    cursor: "#f5e0dc",
    cursor_foreground: "#1e1e2e",
    highlight: "#f5e0dc",
    highlight_foreground: "#1e1e2e",
    ansi: [
        "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de",
        "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8",
    ],
};

pub(super) const ROSE_PINE_TERMINAL_COLORS: TerminalColors = TerminalColors {
    background: "#191724",
    foreground: "#e0def4",
    bold: "#e0def4",
    cursor: "#524f67",
    cursor_foreground: "#e0def4",
    highlight: "#403d52",
    highlight_foreground: "#e0def4",
    ansi: [
        "#26233a", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4",
        "#6e6a86", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4",
    ],
};

pub(super) const TOKYO_NIGHT_TERMINAL_COLORS: TerminalColors = TerminalColors {
    background: "#1a1b26",
    foreground: "#c0caf5",
    bold: "#c0caf5",
    cursor: "#c0caf5",
    cursor_foreground: "#1a1b26",
    highlight: "#283457",
    highlight_foreground: "#c0caf5",
    ansi: [
        "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
        "#414868", "#ff899d", "#9fe044", "#faba4a", "#8db0ff", "#c7a9ff", "#a4daff", "#c0caf5",
    ],
};

pub(super) const DRACULA_TERMINAL_COLORS: TerminalColors = TerminalColors {
    background: "#282a36",
    foreground: "#f8f8f2",
    bold: "#ffffff",
    cursor: "#f8f8f2",
    cursor_foreground: "#282a36",
    highlight: "#44475a",
    highlight_foreground: "#ffffff",
    ansi: [
        "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
        "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
    ],
};

pub(super) const GRUVBOX_DARK_TERMINAL_COLORS: TerminalColors = TerminalColors {
    background: "#282828",
    foreground: "#ebdbb2",
    bold: "#fbf1c7",
    cursor: "#ebdbb2",
    cursor_foreground: "#282828",
    highlight: "#504945",
    highlight_foreground: "#fbf1c7",
    ansi: [
        "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984",
        "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
    ],
};

pub(super) fn terminal_font_description(
    widget: &impl IsA<gtk::Widget>,
    config: &config::AppConfig,
) -> gtk::pango::FontDescription {
    let configured = config.appearance.font_family.trim();
    let family = if configured.is_empty() {
        default_terminal_font_family(&installed_font_families(widget))
    } else {
        configured.to_string()
    };
    terminal_font_description_with_family(config, family)
}

pub(super) fn terminal_font_description_with_family(
    config: &config::AppConfig,
    default_family: String,
) -> gtk::pango::FontDescription {
    let configured = config.appearance.font_family.trim();
    let family = if configured.is_empty() {
        default_family
    } else {
        configured.to_string()
    };
    gtk::pango::FontDescription::from_string(&format!("{} {}", family, config.appearance.font_size))
}

pub(super) fn default_terminal_font_family(installed_families: &[String]) -> String {
    for preferred in PREFERRED_TERMINAL_FONT_FAMILIES {
        if installed_families
            .iter()
            .any(|family| family.eq_ignore_ascii_case(preferred))
        {
            return preferred.to_string();
        }
    }
    installed_families
        .iter()
        .find(|family| family.eq_ignore_ascii_case("monospace"))
        .cloned()
        .unwrap_or_else(|| "monospace".to_string())
}

pub(super) fn installed_font_families(widget: &impl IsA<gtk::Widget>) -> Vec<String> {
    pango_font_families(widget, false)
}

pub(super) fn installed_monospace_font_families(widget: &impl IsA<gtk::Widget>) -> Vec<String> {
    pango_font_families(widget, true)
}

pub(super) fn pango_font_families(
    widget: &impl IsA<gtk::Widget>,
    monospace_only: bool,
) -> Vec<String> {
    let context = widget.as_ref().pango_context();
    let names = context
        .list_families()
        .into_iter()
        .filter(|family| !monospace_only || family.is_monospace())
        .map(|family| family.name().to_string());
    dedupe_font_family_names(names)
}

pub(super) fn resolved_system_monospace_family(widget: &impl IsA<gtk::Widget>) -> Option<String> {
    let context = widget.as_ref().pango_context();
    let description = gtk::pango::FontDescription::from_string("monospace");
    context
        .load_font(&description)
        .and_then(|font| font.describe().family().map(|family| family.to_string()))
        .filter(|family| !family.trim().is_empty())
        .or_else(|| installed_monospace_font_families(widget).into_iter().next())
}

pub(super) fn dedupe_font_family_names(raw_names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut names = BTreeSet::new();
    for name in raw_names {
        let name = name.trim();
        if !name.is_empty() {
            names.insert(name.to_string());
        }
    }
    names.into_iter().collect()
}
