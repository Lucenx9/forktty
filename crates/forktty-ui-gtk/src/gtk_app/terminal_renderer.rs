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
}

impl TerminalRenderer {
    pub(super) fn from_config(config: &config::AppConfig) -> Self {
        Self {
            palette: RendererPalette::from_terminal_colors(terminal_colors_for_config(config)),
        }
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
        cr.move_to(8.0, 18.0);
        let _ = cr.show_text(text);
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
}
