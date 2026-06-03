use super::*;
use forktty_terminal::ghostty::core::{
    TerminalCell, TerminalCellWidth, TerminalFrame, TerminalRgb, TerminalRow,
};
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

    fn from_terminal_rgb(value: TerminalRgb) -> Self {
        Self {
            red: value.red,
            green: value.green,
            blue: value.blue,
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RendererCellStyle {
    foreground: RendererColor,
    background: RendererColor,
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RendererTextRun {
    start_col: usize,
    cell_span: usize,
    text: String,
    foreground: RendererColor,
    background: RendererColor,
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RendererCursorOverlay {
    col: usize,
    row: usize,
    text: String,
    foreground: RendererColor,
    background: RendererColor,
    bold: bool,
    italic: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RendererFrameDefaults {
    foreground: RendererColor,
    background: RendererColor,
}

#[derive(Debug, Clone, Copy)]
struct RendererCellMetrics {
    width: f64,
    height: f64,
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

    pub(super) fn cell_pixel_size_for_widget(&self, widget: &impl IsA<gtk::Widget>) -> (i32, i32) {
        let context = widget.as_ref().pango_context();
        let metrics = context.metrics(Some(&self.font), None::<&gtk::pango::Language>);
        let width = (metrics.approximate_char_width() / gtk::pango::SCALE).max(1);
        let height = ((metrics.ascent() + metrics.descent()) / gtk::pango::SCALE).max(1);
        (width, height)
    }

    pub(super) fn draw_frame(
        &self,
        cr: &gtk::cairo::Context,
        width: i32,
        height: i32,
        frame: &TerminalFrame,
    ) {
        let defaults = self.frame_defaults(frame);
        let default_background = defaults.background;
        default_background.set_cairo_source(cr);
        cr.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
        let _ = cr.fill();
        let metrics = self.cell_metrics(cr);

        for (row_idx, row) in frame.rows.iter().enumerate() {
            let y = row_idx as f64 * metrics.height;
            for (col_idx, cell) in row.cells.iter().enumerate() {
                let background = self.cell_background_for_frame(frame, cell);
                if background != default_background {
                    background.set_cairo_source(cr);
                    cr.rectangle(
                        col_idx as f64 * metrics.width,
                        y,
                        metrics.width,
                        metrics.height,
                    );
                    let _ = cr.fill();
                }
            }

            for run in self.text_runs_for_frame_row(frame, row) {
                run.foreground.set_cairo_source(cr);
                let layout = pangocairo::functions::create_layout(cr);
                let mut font = self.font.clone();
                if run.bold {
                    font.set_weight(gtk::pango::Weight::Bold);
                }
                if run.italic {
                    font.set_style(gtk::pango::Style::Italic);
                }
                layout.set_font_description(Some(&font));
                layout.set_text(&run.text);
                cr.move_to(run.start_col as f64 * metrics.width, y);
                pangocairo::functions::show_layout(cr, &layout);
                self.draw_text_decorations(cr, &run, metrics, y);
            }
        }

        if let Some(cursor) = self.cursor_overlay_for_frame(frame) {
            self.draw_cursor_overlay(cr, &cursor, metrics);
        }
    }

    #[cfg(test)]
    fn text_runs_for_row(&self, row: &TerminalRow) -> Vec<RendererTextRun> {
        self.text_runs_for_row_with_defaults(row, self.palette_defaults())
    }

    fn text_runs_for_frame_row(
        &self,
        frame: &TerminalFrame,
        row: &TerminalRow,
    ) -> Vec<RendererTextRun> {
        self.text_runs_for_row_with_defaults(row, self.frame_defaults(frame))
    }

    fn text_runs_for_row_with_defaults(
        &self,
        row: &TerminalRow,
        defaults: RendererFrameDefaults,
    ) -> Vec<RendererTextRun> {
        let mut runs = Vec::new();
        let mut current: Option<RendererTextRun> = None;
        for (col, cell) in row.cells.iter().enumerate() {
            if !cell_renders_text(cell) {
                if let Some(run) = current.take() {
                    runs.push(run);
                }
                continue;
            }
            let style = RendererCellStyle {
                foreground: self.cell_foreground_with_defaults(cell, defaults),
                background: self.cell_background_with_defaults(cell, defaults),
                bold: cell.bold,
                italic: cell.italic,
                underline: cell.underline,
                strikethrough: cell.strikethrough,
            };
            let cell_span = cell_grid_span(cell);
            match &mut current {
                Some(run) if run.style() == style => {
                    run.text.push_str(&cell.text);
                    run.cell_span += cell_span;
                }
                Some(_) => {
                    runs.push(current.take().expect("current run is present"));
                    current = Some(RendererTextRun::from_cell(
                        col, &cell.text, style, cell_span,
                    ));
                }
                None => {
                    current = Some(RendererTextRun::from_cell(
                        col, &cell.text, style, cell_span,
                    ))
                }
            }
        }
        if let Some(run) = current {
            runs.push(run);
        }
        runs
    }

    fn cursor_overlay_for_frame(&self, frame: &TerminalFrame) -> Option<RendererCursorOverlay> {
        let cursor = frame.cursor.filter(|cursor| cursor.visible)?;
        let row = frame.rows.get(usize::from(cursor.y))?;
        let cell = row.cells.get(usize::from(cursor.x))?;
        let text = if cell_renders_text(cell) {
            cell.text.clone()
        } else {
            String::new()
        };
        Some(RendererCursorOverlay {
            col: usize::from(cursor.x),
            row: usize::from(cursor.y),
            text,
            foreground: self.palette.cursor_foreground,
            background: self.palette.cursor,
            bold: cell.bold,
            italic: cell.italic,
        })
    }

    #[cfg(test)]
    fn cell_foreground(&self, cell: &TerminalCell) -> RendererColor {
        self.cell_foreground_with_defaults(cell, self.palette_defaults())
    }

    fn cell_foreground_with_defaults(
        &self,
        cell: &TerminalCell,
        defaults: RendererFrameDefaults,
    ) -> RendererColor {
        let default_foreground = if cell.bold {
            self.palette.bold
        } else {
            defaults.foreground
        };
        let mut foreground = cell
            .foreground
            .map_or(default_foreground, RendererColor::from_terminal_rgb);
        let mut background = cell
            .background
            .map_or(defaults.background, RendererColor::from_terminal_rgb);
        if cell.inverse {
            std::mem::swap(&mut foreground, &mut background);
        }
        foreground
    }

    #[cfg(test)]
    fn cell_background(&self, cell: &TerminalCell) -> RendererColor {
        self.cell_background_with_defaults(cell, self.palette_defaults())
    }

    fn cell_background_for_frame(
        &self,
        frame: &TerminalFrame,
        cell: &TerminalCell,
    ) -> RendererColor {
        self.cell_background_with_defaults(cell, self.frame_defaults(frame))
    }

    fn cell_background_with_defaults(
        &self,
        cell: &TerminalCell,
        defaults: RendererFrameDefaults,
    ) -> RendererColor {
        let default_foreground = if cell.bold {
            self.palette.bold
        } else {
            defaults.foreground
        };
        let mut foreground = cell
            .foreground
            .map_or(default_foreground, RendererColor::from_terminal_rgb);
        let mut background = cell
            .background
            .map_or(defaults.background, RendererColor::from_terminal_rgb);
        if cell.inverse {
            std::mem::swap(&mut foreground, &mut background);
        }
        background
    }

    #[cfg(test)]
    fn palette_defaults(&self) -> RendererFrameDefaults {
        RendererFrameDefaults {
            foreground: self.palette.foreground,
            background: self.palette.background,
        }
    }

    fn frame_defaults(&self, frame: &TerminalFrame) -> RendererFrameDefaults {
        RendererFrameDefaults {
            foreground: RendererColor::from_terminal_rgb(frame.foreground),
            background: RendererColor::from_terminal_rgb(frame.background),
        }
    }

    fn cell_metrics(&self, cr: &gtk::cairo::Context) -> RendererCellMetrics {
        let layout = pangocairo::functions::create_layout(cr);
        layout.set_font_description(Some(&self.font));
        layout.set_text("W");
        let (_ink, logical) = layout.pixel_extents();
        RendererCellMetrics {
            width: f64::from(logical.width().max(1)),
            height: f64::from(logical.height().max(1)),
        }
    }

    fn draw_text_decorations(
        &self,
        cr: &gtk::cairo::Context,
        run: &RendererTextRun,
        metrics: RendererCellMetrics,
        y: f64,
    ) {
        let x = run.start_col as f64 * metrics.width;
        let width = run.cell_span as f64 * metrics.width;
        if run.underline {
            cr.move_to(x, y + metrics.height - 2.0);
            cr.line_to(x + width, y + metrics.height - 2.0);
            let _ = cr.stroke();
        }
        if run.strikethrough {
            cr.move_to(x, y + metrics.height * 0.58);
            cr.line_to(x + width, y + metrics.height * 0.58);
            let _ = cr.stroke();
        }
    }

    fn draw_cursor_overlay(
        &self,
        cr: &gtk::cairo::Context,
        cursor: &RendererCursorOverlay,
        metrics: RendererCellMetrics,
    ) {
        let x = cursor.col as f64 * metrics.width;
        let y = cursor.row as f64 * metrics.height;
        cursor.background.set_cairo_source(cr);
        cr.rectangle(x, y, metrics.width, metrics.height);
        let _ = cr.fill();

        if cursor.text.is_empty() {
            return;
        }

        cursor.foreground.set_cairo_source(cr);
        let layout = pangocairo::functions::create_layout(cr);
        let mut font = self.font.clone();
        if cursor.bold {
            font.set_weight(gtk::pango::Weight::Bold);
        }
        if cursor.italic {
            font.set_style(gtk::pango::Style::Italic);
        }
        layout.set_font_description(Some(&font));
        layout.set_text(&cursor.text);
        cr.move_to(x, y);
        pangocairo::functions::show_layout(cr, &layout);
    }
}

impl RendererTextRun {
    fn from_cell(start_col: usize, text: &str, style: RendererCellStyle, cell_span: usize) -> Self {
        Self {
            start_col,
            cell_span,
            text: text.to_string(),
            foreground: style.foreground,
            background: style.background,
            bold: style.bold,
            italic: style.italic,
            underline: style.underline,
            strikethrough: style.strikethrough,
        }
    }

    fn style(&self) -> RendererCellStyle {
        RendererCellStyle {
            foreground: self.foreground,
            background: self.background,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strikethrough: self.strikethrough,
        }
    }
}

fn cell_renders_text(cell: &TerminalCell) -> bool {
    !cell.text.is_empty()
        && !cell.invisible
        && !matches!(
            cell.width,
            TerminalCellWidth::SpacerTail | TerminalCellWidth::SpacerHead
        )
}

fn cell_grid_span(cell: &TerminalCell) -> usize {
    match cell.width {
        TerminalCellWidth::Wide => 2,
        TerminalCellWidth::Narrow
        | TerminalCellWidth::SpacerHead
        | TerminalCellWidth::SpacerTail => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forktty_terminal::ghostty::core::{TerminalCell, TerminalRgb, TerminalRow};

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

    #[test]
    fn terminal_renderer_groups_cells_by_visual_style() {
        let config = config::AppConfig::default();
        let renderer = TerminalRenderer::from_config_with_font(
            &config,
            gtk::pango::FontDescription::from_string("monospace 12"),
        );
        let red = TerminalRgb {
            red: 255,
            green: 0,
            blue: 0,
        };
        let default_fg = TerminalRgb {
            red: 215,
            green: 215,
            blue: 215,
        };
        let default_bg = TerminalRgb {
            red: 24,
            green: 24,
            blue: 24,
        };
        let row = TerminalRow {
            cells: vec![
                test_cell("r", Some(red), None),
                test_cell("e", Some(red), None),
                test_cell("d", Some(red), None),
                test_cell(" ", None, None),
                test_cell("o", None, None),
                test_cell("k", None, None),
            ],
        };

        let runs = renderer.text_runs_for_row(&row);

        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].start_col, 0);
        assert_eq!(runs[0].text, "red");
        assert_eq!(runs[0].foreground, RendererColor::from_terminal_rgb(red));
        assert_eq!(runs[1].start_col, 3);
        assert_eq!(runs[1].text, " ok");
        assert_eq!(
            runs[1].foreground,
            RendererColor::from_terminal_rgb(default_fg)
        );
        assert_eq!(
            runs[1].background,
            RendererColor::from_terminal_rgb(default_bg)
        );
    }

    #[test]
    fn terminal_renderer_resolves_default_cells_from_config_palette() {
        let mut config = config::AppConfig::default();
        config.appearance.terminal_theme = config::TERMINAL_THEME_DRACULA.to_string();
        let renderer = TerminalRenderer::from_config_with_font(
            &config,
            gtk::pango::FontDescription::from_string("monospace 12"),
        );
        let default_cell = test_cell("x", None, None);

        assert_eq!(
            renderer.cell_background(&default_cell).to_string(),
            DRACULA_TERMINAL_COLORS.background
        );
        assert_eq!(
            renderer.cell_foreground(&default_cell).to_string(),
            DRACULA_TERMINAL_COLORS.foreground
        );
    }

    #[test]
    fn terminal_renderer_resolves_default_cells_from_frame_colors() {
        let config = config::AppConfig::default();
        let renderer = TerminalRenderer::from_config_with_font(
            &config,
            gtk::pango::FontDescription::from_string("monospace 12"),
        );
        let foreground = TerminalRgb {
            red: 0x11,
            green: 0x22,
            blue: 0x33,
        };
        let background = TerminalRgb {
            red: 0x44,
            green: 0x55,
            blue: 0x66,
        };
        let frame = test_frame(
            foreground,
            background,
            TerminalRow {
                cells: vec![test_cell("x", None, None)],
            },
        );

        let runs = renderer.text_runs_for_frame_row(&frame, &frame.rows[0]);

        assert_eq!(
            runs[0].foreground,
            RendererColor::from_terminal_rgb(foreground)
        );
        assert_eq!(
            runs[0].background,
            RendererColor::from_terminal_rgb(background)
        );
        assert_eq!(
            renderer
                .cell_background_for_frame(&frame, &frame.rows[0].cells[0])
                .to_string(),
            "#445566"
        );
    }

    #[test]
    fn terminal_renderer_uses_bold_palette_for_default_bold_cells() {
        let config = config::AppConfig::default();
        let renderer = TerminalRenderer::from_config_with_font(
            &config,
            gtk::pango::FontDescription::from_string("monospace 12"),
        );
        let mut bold_cell = test_cell("x", None, None);
        bold_cell.bold = true;

        assert_eq!(
            renderer.cell_foreground(&bold_cell).to_string(),
            FORKTTY_DARK_TERMINAL_COLORS.bold
        );
    }

    #[test]
    fn terminal_renderer_does_not_emit_text_runs_for_invisible_cells() {
        let config = config::AppConfig::default();
        let renderer = TerminalRenderer::from_config_with_font(
            &config,
            gtk::pango::FontDescription::from_string("monospace 12"),
        );
        let mut invisible_cell = test_cell("x", None, None);
        invisible_cell.invisible = true;
        let row = TerminalRow {
            cells: vec![invisible_cell],
        };

        assert!(renderer.text_runs_for_row(&row).is_empty());
    }

    #[test]
    fn terminal_renderer_does_not_emit_text_runs_for_wide_spacer_cells() {
        let config = config::AppConfig::default();
        let renderer = TerminalRenderer::from_config_with_font(
            &config,
            gtk::pango::FontDescription::from_string("monospace 12"),
        );
        let mut spacer = test_cell("x", None, None);
        spacer.width = TerminalCellWidth::SpacerTail;
        let row = TerminalRow {
            cells: vec![spacer],
        };

        assert!(renderer.text_runs_for_row(&row).is_empty());
    }

    #[test]
    fn terminal_renderer_tracks_grid_span_for_wide_and_combining_cells() {
        let config = config::AppConfig::default();
        let renderer = TerminalRenderer::from_config_with_font(
            &config,
            gtk::pango::FontDescription::from_string("monospace 12"),
        );
        let mut wide = test_cell("橋", None, None);
        wide.width = TerminalCellWidth::Wide;
        let mut spacer = test_cell("", None, None);
        spacer.width = TerminalCellWidth::SpacerTail;
        let combining = test_cell("e\u{301}", None, None);
        let row = TerminalRow {
            cells: vec![wide, spacer, combining],
        };

        let runs = renderer.text_runs_for_row(&row);

        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "橋");
        assert_eq!(runs[0].cell_span, 2);
        assert_eq!(runs[1].text, "e\u{301}");
        assert_eq!(runs[1].cell_span, 1);
    }

    #[test]
    fn terminal_renderer_builds_block_cursor_overlay_from_cell_text() {
        let config = config::AppConfig::default();
        let renderer = TerminalRenderer::from_config_with_font(
            &config,
            gtk::pango::FontDescription::from_string("monospace 12"),
        );
        let frame = test_frame(
            TerminalRgb {
                red: 0xd7,
                green: 0xd7,
                blue: 0xd7,
            },
            TerminalRgb {
                red: 0x18,
                green: 0x18,
                blue: 0x18,
            },
            TerminalRow {
                cells: vec![test_cell("a", None, None), test_cell("b", None, None)],
            },
        )
        .with_cursor(1, 0);

        let overlay = renderer.cursor_overlay_for_frame(&frame).unwrap();

        assert_eq!(overlay.col, 1);
        assert_eq!(overlay.row, 0);
        assert_eq!(overlay.text, "b");
        assert_eq!(overlay.background, renderer.palette.cursor);
        assert_eq!(overlay.foreground, renderer.palette.cursor_foreground);
    }

    fn test_cell(
        text: &str,
        foreground: Option<TerminalRgb>,
        background: Option<TerminalRgb>,
    ) -> TerminalCell {
        TerminalCell {
            text: text.to_string(),
            foreground,
            background,
            width: TerminalCellWidth::Narrow,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            inverse: false,
            invisible: false,
        }
    }

    fn test_frame(
        foreground: TerminalRgb,
        background: TerminalRgb,
        row: TerminalRow,
    ) -> TerminalFrame {
        TerminalFrame {
            cols: row.cells.len() as u16,
            row_count: 1,
            background,
            foreground,
            cursor: None,
            rows: vec![row],
        }
    }

    trait TestFrameCursor {
        fn with_cursor(self, x: u16, y: u16) -> Self;
    }

    impl TestFrameCursor for TerminalFrame {
        fn with_cursor(mut self, x: u16, y: u16) -> Self {
            self.cursor = Some(forktty_terminal::ghostty::core::TerminalCursor {
                x,
                y,
                visible: true,
            });
            self
        }
    }
}
