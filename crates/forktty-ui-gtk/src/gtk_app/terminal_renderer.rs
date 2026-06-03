use super::*;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RendererColor {
    red: u8,
    green: u8,
    blue: u8,
}

impl RendererColor {
    fn parse(value: &str) -> Self {
        let value = value.trim().strip_prefix('#').unwrap_or(value.trim());
        if value.len() != 6 {
            return Self::BLACK;
        }
        let parse_pair = |idx| u8::from_str_radix(&value[idx..idx + 2], 16).unwrap_or(0);
        Self {
            red: parse_pair(0),
            green: parse_pair(2),
            blue: parse_pair(4),
        }
    }

    fn set_cairo_source(self, cr: &gtk::cairo::Context) {
        cr.set_source_rgb(
            f64::from(self.red) / 255.0,
            f64::from(self.green) / 255.0,
            f64::from(self.blue) / 255.0,
        );
    }

    const BLACK: Self = Self {
        red: 0,
        green: 0,
        blue: 0,
    };
}

impl fmt::Display for RendererColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.red, self.green, self.blue)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RendererPalette {
    pub(super) background: RendererColor,
    pub(super) foreground: RendererColor,
    pub(super) bold: RendererColor,
    pub(super) cursor: RendererColor,
    pub(super) cursor_foreground: RendererColor,
    pub(super) highlight: RendererColor,
    pub(super) highlight_foreground: RendererColor,
    pub(super) ansi: Vec<RendererColor>,
}

impl RendererPalette {
    pub(super) fn from_terminal_colors(colors: &TerminalColors) -> Self {
        Self {
            background: RendererColor::parse(colors.background),
            foreground: RendererColor::parse(colors.foreground),
            bold: RendererColor::parse(colors.bold),
            cursor: RendererColor::parse(colors.cursor),
            cursor_foreground: RendererColor::parse(colors.cursor_foreground),
            highlight: RendererColor::parse(colors.highlight),
            highlight_foreground: RendererColor::parse(colors.highlight_foreground),
            ansi: colors
                .ansi
                .iter()
                .map(|color| RendererColor::parse(color))
                .collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct TerminalRenderer {
    palette: RendererPalette,
    font: gtk::pango::FontDescription,
}

impl TerminalRenderer {
    pub(super) fn from_config_with_font(
        config: &config::AppConfig,
        font: gtk::pango::FontDescription,
    ) -> Self {
        Self {
            palette: RendererPalette::from_terminal_colors(terminal_colors_for_config(config)),
            font,
        }
    }

    #[cfg(test)]
    pub(super) fn font_description(&self) -> gtk::pango::FontDescription {
        self.font.clone()
    }

    pub(super) fn draw_plain_text(
        &self,
        cr: &gtk::cairo::Context,
        width: i32,
        height: i32,
        text: &str,
    ) {
        self.palette.background.set_cairo_source(cr);
        cr.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
        let _ = cr.fill();
        self.palette.foreground.set_cairo_source(cr);
        let layout = pangocairo::functions::create_layout(cr);
        layout.set_font_description(Some(&self.font));
        layout.set_text(text);
        layout.set_width((width.saturating_sub(16).max(1)) * gtk::pango::SCALE);
        layout.set_wrap(gtk::pango::WrapMode::Char);
        cr.move_to(8.0, 8.0);
        pangocairo::functions::show_layout(cr, &layout);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_renderer_maps_theme_colors_to_ansi_palette() {
        let config = config::AppConfig::default();
        let palette = RendererPalette::from_terminal_colors(terminal_colors_for_config(&config));

        assert_eq!(palette.ansi.len(), 16);
        assert_eq!(palette.background.to_string(), "#181818");
    }

    #[test]
    fn terminal_renderer_uses_configured_terminal_font() {
        let config = config::AppConfig::default();
        let font = gtk::pango::FontDescription::from_string("JetBrainsMono Nerd Font Mono 13");
        let renderer = TerminalRenderer::from_config_with_font(&config, font.clone());

        assert_eq!(renderer.font_description(), font);
    }
}
