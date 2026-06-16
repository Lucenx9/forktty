use super::terminal_geometry::{terminal_grid_geometry, TerminalGridGeometry, TERMINAL_PADDING_PX};
use super::*;
use forktty_terminal::ghostty::core::{
    TerminalCell, TerminalCellWidth, TerminalCursorStyle, TerminalFrame, TerminalRgb, TerminalRow,
    TerminalViewportPosition,
};
use std::{f64::consts::PI, fmt};

/// Ghostty-style inactive split dim: enough to clarify focus without making
/// background panes unreadable.
const UNFOCUSED_SPLIT_DIM_ALPHA: f64 = 0.08;
const SCROLLBACK_INDICATOR_WIDTH_PX: f64 = 4.0;
const SCROLLBACK_INDICATOR_ALPHA: f64 = 0.25;
const VISUAL_BELL_BORDER_PX: f64 = 2.0;
const TERMINAL_CELL_WIDTH_PROBE: &str = "W";
const TERMINAL_CELL_HEIGHT_PROBE: &str = "Wgjpqy█▁▂▃▄▅▆▇╭─╮│╰─╯┼╬╠╣║";
const TERMINAL_GLYPH_GUARD_PX: i32 = 1;

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
        let parse_pair = |range: std::ops::Range<usize>| {
            value
                .get(range)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .unwrap_or(0)
        };
        Self {
            red: parse_pair(0..2),
            green: parse_pair(2..4),
            blue: parse_pair(4..6),
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

// Mirrors the brand accent in style.css (`@define-color accent_color`);
// keep the two in sync when retheming. CSS can't be shared here because
// GTK 4.14 drops custom properties.
const VISUAL_BELL_ACCENT: RendererColor = RendererColor {
    red: 0xe8,
    green: 0x87,
    blue: 0x45,
};

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
    pub(super) bold_is_bright: bool,
    pub(super) cursor: RendererColor,
    pub(super) cursor_foreground: RendererColor,
    pub(super) highlight: RendererColor,
    pub(super) highlight_foreground: RendererColor,
    pub(super) ansi: Vec<RendererColor>,
}

impl RendererPalette {
    pub(super) fn from_terminal_colors(colors: &TerminalColors) -> Self {
        Self {
            background: RendererColor::parse(&colors.background),
            foreground: RendererColor::parse(&colors.foreground),
            bold: RendererColor::parse(&colors.bold),
            bold_is_bright: colors.bold_is_bright,
            cursor: RendererColor::parse(&colors.cursor),
            cursor_foreground: RendererColor::parse(&colors.cursor_foreground),
            highlight: RendererColor::parse(&colors.highlight),
            highlight_foreground: RendererColor::parse(&colors.highlight_foreground),
            ansi: colors
                .ansi
                .iter()
                .map(|color| RendererColor::parse(color))
                .collect(),
        }
    }
}

#[cfg(test)]
fn renderer_palette_ansi_defaults(palette: &RendererPalette) -> [RendererColor; 16] {
    std::array::from_fn(|index| {
        palette
            .ansi
            .get(index)
            .copied()
            .unwrap_or(palette.foreground)
    })
}

#[derive(Debug, Clone)]
pub(super) struct TerminalRenderer {
    palette: RendererPalette,
    font: gtk::pango::FontDescription,
    bold_font: Option<gtk::pango::FontDescription>,
    italic_font: Option<gtk::pango::FontDescription>,
    bold_italic_font: Option<gtk::pango::FontDescription>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RendererBackgroundRun {
    start_col: usize,
    cell_span: usize,
    background: RendererColor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RendererCursorOverlay {
    col: usize,
    row: usize,
    cell_span: usize,
    style: RendererCursorStyle,
    text: String,
    foreground: RendererColor,
    background: RendererColor,
    bold: bool,
    italic: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RendererCursorStyle {
    Bar,
    Block,
    Underline,
    BlockHollow,
}

impl From<TerminalCursorStyle> for RendererCursorStyle {
    fn from(value: TerminalCursorStyle) -> Self {
        match value {
            TerminalCursorStyle::Bar => Self::Bar,
            TerminalCursorStyle::Block => Self::Block,
            TerminalCursorStyle::Underline => Self::Underline,
            TerminalCursorStyle::BlockHollow => Self::BlockHollow,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RendererFrameDefaults {
    foreground: RendererColor,
    background: RendererColor,
    palette: [RendererColor; 16],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScrollbackIndicatorGeometry {
    y: i32,
    height: i32,
}

/// How the cursor overlay should be drawn for the current frame: unfocused
/// panes get a steady hollow cursor, focused panes blink with `blink_visible`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RendererCursorState {
    pub(super) focused: bool,
    pub(super) blink_visible: bool,
    pub(super) has_siblings: bool,
    /// Whether the model considers this pane focused. GTK focus leaves every
    /// drawing area when a search entry, the command palette, or a dialog is
    /// active; the model focus keeps the logically active pane undimmed.
    pub(super) model_focused: bool,
    pub(super) visual_bell_active: bool,
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
        let variants = terminal_font_variants_for_config(config, &font);
        Self {
            palette: RendererPalette::from_terminal_colors(&terminal_colors_for_config(config)),
            font,
            bold_font: variants.bold,
            italic_font: variants.italic,
            bold_italic_font: variants.bold_italic,
        }
    }

    #[cfg(test)]
    pub(super) fn font_description(&self) -> gtk::pango::FontDescription {
        self.font.clone()
    }

    #[cfg(test)]
    fn font_for_style(&self, bold: bool, italic: bool) -> gtk::pango::FontDescription {
        self.font_for_cell_style(bold, italic)
    }

    /// Cell size used for pixel<->cell mapping (mouse, selection, resize).
    /// Drawing receives this same size from the widget path; using a separate
    /// Cairo measurement can leave the PTY one row taller than the painted grid.
    pub(super) fn cell_pixel_size_for_widget(&self, widget: &impl IsA<gtk::Widget>) -> (i32, i32) {
        let width_layout = widget
            .as_ref()
            .create_pango_layout(Some(TERMINAL_CELL_WIDTH_PROBE));
        width_layout.set_font_description(Some(&self.font));
        let (_width_ink, width_logical) = width_layout.pixel_extents();
        let height_layout = widget
            .as_ref()
            .create_pango_layout(Some(TERMINAL_CELL_HEIGHT_PROBE));
        height_layout.set_font_description(Some(&self.font));
        let (_height_ink, height_logical) = height_layout.pixel_extents();
        terminal_cell_pixel_size(width_logical, height_logical)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_frame(
        &self,
        cr: &gtk::cairo::Context,
        width: i32,
        height: i32,
        cell_size: (i32, i32),
        frame: &TerminalFrame,
        selection: Option<(SelectionPoint, SelectionPoint)>,
        hover_link: Option<(SelectionPoint, SelectionPoint)>,
        cursor_state: RendererCursorState,
        viewport: Option<TerminalViewportPosition>,
    ) {
        let defaults = self.frame_defaults(frame);
        let default_background = defaults.background;
        default_background.set_cairo_source(cr);
        cr.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
        let _ = cr.fill();
        let metrics = renderer_cell_metrics_from_size(cell_size);
        let grid = terminal_grid_geometry(
            width,
            height,
            frame.cols,
            frame.row_count,
            metrics.width as i32,
            metrics.height as i32,
        );
        let origin_x = f64::from(grid.origin_x);
        let origin_y = f64::from(grid.origin_y);

        for (row_idx, row) in frame.rows.iter().enumerate() {
            let y = origin_y + row_idx as f64 * metrics.height;
            let selected_span = selection
                .and_then(|(start, end)| {
                    selection_cols_for_row(start, end, row_idx, row.cells.len())
                })
                .map(|(from, to)| {
                    (
                        origin_x + from as f64 * metrics.width,
                        (to - from) as f64 * metrics.width,
                    )
                });
            let link_cols = hover_link.and_then(|(start, end)| {
                selection_cols_for_row(start, end, row_idx, row.cells.len())
            });
            for background in self.background_runs_for_frame_row(frame, row) {
                background.background.set_cairo_source(cr);
                cr.rectangle(
                    origin_x + background.start_col as f64 * metrics.width,
                    y,
                    background.cell_span as f64 * metrics.width,
                    metrics.height,
                );
                let _ = cr.fill();
            }

            // Selection/search highlight: an opaque theme-highlight background
            // painted under the glyphs, with the selected glyphs repainted in
            // the theme's paired highlight foreground. A translucent overlay on
            // top of the text was barely visible on dark themes and dimmed the
            // glyphs as soon as it was opaque enough to see.
            if let Some((x, width)) = selected_span {
                self.palette.highlight.set_cairo_source(cr);
                cr.rectangle(x, y, width, metrics.height);
                let _ = cr.fill();
            }

            self.draw_row_text(cr, frame, row, metrics, origin_x, y, None, link_cols);

            if let Some((x, width)) = selected_span {
                let _ = cr.save();
                cr.rectangle(x, y, width, metrics.height);
                cr.clip();
                self.draw_row_text(
                    cr,
                    frame,
                    row,
                    metrics,
                    origin_x,
                    y,
                    Some(self.palette.highlight_foreground),
                    link_cols,
                );
                let _ = cr.restore();
            }
        }

        if let Some(mut cursor) = self.cursor_overlay_for_frame(frame) {
            if !cursor_state.focused {
                // Unfocused panes show a hollow cursor so the pane that will
                // receive keystrokes is unambiguous; it never blinks.
                cursor.style = RendererCursorStyle::BlockHollow;
            } else if !cursor_state.blink_visible {
                // Hidden half of the focused cursor's blink cycle.
                cursor.cell_span = 0;
            }
            if cursor.cell_span > 0 {
                self.draw_cursor_overlay(cr, &cursor, metrics, grid);
            }
        }
        if should_dim_pane(
            cursor_state.focused,
            cursor_state.has_siblings,
            cursor_state.model_focused,
        ) {
            paint_unfocused_split_dim(cr, width, height);
        }
        if let Some(viewport) = viewport {
            if let Some(indicator) =
                scrollback_indicator_geometry(viewport.top, viewport.rows, viewport.total, height)
            {
                paint_scrollback_indicator(cr, width, indicator);
            }
        }
        if cursor_state.visual_bell_active {
            paint_visual_bell_border(cr, width, height);
        }
    }

    #[cfg(test)]
    fn text_runs_for_row(&self, row: &TerminalRow) -> Vec<RendererTextRun> {
        self.text_runs_for_row_with_defaults(row, self.palette_defaults(), None)
    }

    fn background_runs_for_frame_row(
        &self,
        frame: &TerminalFrame,
        row: &TerminalRow,
    ) -> Vec<RendererBackgroundRun> {
        let default_background = self.frame_defaults(frame).background;
        row.cells
            .iter()
            .enumerate()
            .filter_map(|(col, cell)| {
                if matches!(
                    cell.width,
                    TerminalCellWidth::SpacerTail | TerminalCellWidth::SpacerHead
                ) {
                    return None;
                }
                let background = self.cell_background_for_frame(frame, cell);
                if background == default_background {
                    return None;
                }
                Some(RendererBackgroundRun {
                    start_col: col,
                    cell_span: cell_grid_span(cell),
                    background,
                })
            })
            .collect()
    }

    fn text_runs_for_frame_row(
        &self,
        frame: &TerminalFrame,
        row: &TerminalRow,
        link_cols: Option<(usize, usize)>,
    ) -> Vec<RendererTextRun> {
        self.text_runs_for_row_with_defaults(row, self.frame_defaults(frame), link_cols)
    }

    fn text_runs_for_row_with_defaults(
        &self,
        row: &TerminalRow,
        defaults: RendererFrameDefaults,
        link_cols: Option<(usize, usize)>,
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
                underline: cell.underline
                    || cell.hyperlink
                    || link_cols.is_some_and(|(from, to)| col >= from && col < to),
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
        let mut col = usize::from(cursor.x);
        if cursor.at_wide_tail && col > 0 {
            col -= 1;
        }
        let cell = row.cells.get(col)?;
        let text = if cell_renders_text(cell) {
            cell.text.clone()
        } else {
            String::new()
        };
        Some(RendererCursorOverlay {
            col,
            row: usize::from(cursor.y),
            cell_span: cell_grid_span(cell),
            style: RendererCursorStyle::from(cursor.style),
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

    #[cfg(test)]
    fn cell_foreground_for_frame(
        &self,
        frame: &TerminalFrame,
        cell: &TerminalCell,
    ) -> RendererColor {
        self.cell_foreground_with_defaults(cell, self.frame_defaults(frame))
    }

    fn cell_foreground_with_defaults(
        &self,
        cell: &TerminalCell,
        defaults: RendererFrameDefaults,
    ) -> RendererColor {
        let default_foreground = if cell.bold && !self.palette.bold_is_bright {
            self.palette.bold
        } else {
            defaults.foreground
        };
        let mut foreground = cell
            .foreground
            .map_or(default_foreground, RendererColor::from_terminal_rgb);
        if cell.bold && self.palette.bold_is_bright {
            if let Some(index) = cell
                .foreground_palette
                .filter(|index| *index < 8)
                .map(|index| usize::from(index) + 8)
            {
                foreground = defaults.palette[index];
            }
        }
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
            palette: renderer_palette_ansi_defaults(&self.palette),
        }
    }

    fn frame_defaults(&self, frame: &TerminalFrame) -> RendererFrameDefaults {
        RendererFrameDefaults {
            foreground: RendererColor::from_terminal_rgb(frame.foreground),
            background: RendererColor::from_terminal_rgb(frame.background),
            palette: frame.palette.map(RendererColor::from_terminal_rgb),
        }
    }

    /// Draws one row's text runs (glyphs plus decorations). With a `color`
    /// override every run paints in that color instead of its own foreground —
    /// used clipped to the selection span to repaint selected glyphs in the
    /// theme's highlight foreground.
    #[allow(clippy::too_many_arguments)]
    fn draw_row_text(
        &self,
        cr: &gtk::cairo::Context,
        frame: &TerminalFrame,
        row: &TerminalRow,
        metrics: RendererCellMetrics,
        origin_x: f64,
        y: f64,
        color: Option<RendererColor>,
        link_cols: Option<(usize, usize)>,
    ) {
        for run in self.text_runs_for_frame_row(frame, row, link_cols) {
            color.unwrap_or(run.foreground).set_cairo_source(cr);
            let layout = pangocairo::functions::create_layout(cr);
            let font = self.font_for_cell_style(run.bold, run.italic);
            layout.set_font_description(Some(&font));
            layout.set_text(&run.text);
            draw_layout_fitted_to_cells(
                cr,
                &layout,
                origin_x + run.start_col as f64 * metrics.width,
                y,
                run.cell_span as f64 * metrics.width,
            );
            self.draw_text_decorations(cr, &run, metrics, origin_x, y);
        }
    }

    fn draw_text_decorations(
        &self,
        cr: &gtk::cairo::Context,
        run: &RendererTextRun,
        metrics: RendererCellMetrics,
        origin_x: f64,
        y: f64,
    ) {
        let x = origin_x + run.start_col as f64 * metrics.width;
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
        grid: TerminalGridGeometry,
    ) {
        let x = f64::from(grid.origin_x) + cursor.col as f64 * metrics.width;
        let y = f64::from(grid.origin_y) + cursor.row as f64 * metrics.height;
        cursor.background.set_cairo_source(cr);
        let width = cursor.cell_span as f64 * metrics.width;
        match cursor.style {
            RendererCursorStyle::Bar => {
                cr.rectangle(x, y, 2.0_f64.min(width), metrics.height);
                let _ = cr.fill();
                return;
            }
            RendererCursorStyle::Underline => {
                let underline_height = 2.0_f64.max(metrics.height * 0.12);
                cr.rectangle(
                    x,
                    y + metrics.height - underline_height,
                    width,
                    underline_height,
                );
                let _ = cr.fill();
                return;
            }
            RendererCursorStyle::BlockHollow => {
                cr.rectangle(x + 0.5, y + 0.5, width - 1.0, metrics.height - 1.0);
                let _ = cr.stroke();
                return;
            }
            RendererCursorStyle::Block => {
                cr.rectangle(x, y, width, metrics.height);
                let _ = cr.fill();
            }
        }

        if cursor.text.is_empty() {
            return;
        }

        cursor.foreground.set_cairo_source(cr);
        let layout = pangocairo::functions::create_layout(cr);
        let font = self.font_for_cell_style(cursor.bold, cursor.italic);
        layout.set_font_description(Some(&font));
        layout.set_text(&cursor.text);
        draw_layout_fitted_to_cells(cr, &layout, x, y, width);
    }

    fn font_for_cell_style(&self, bold: bool, italic: bool) -> gtk::pango::FontDescription {
        if bold && italic {
            if let Some(font) = &self.bold_italic_font {
                return font.clone();
            }
        }
        if bold {
            if let Some(font) = &self.bold_font {
                return font.clone();
            }
        }
        if italic {
            if let Some(font) = &self.italic_font {
                return font.clone();
            }
        }

        let mut font = self.font.clone();
        if bold {
            font.set_weight(gtk::pango::Weight::Bold);
        }
        if italic {
            font.set_style(gtk::pango::Style::Italic);
        }
        font
    }
}

fn draw_layout_fitted_to_cells(
    cr: &gtk::cairo::Context,
    layout: &gtk::pango::Layout,
    x: f64,
    y: f64,
    target_width: f64,
) {
    let (natural_width, _) = layout.pixel_size();
    let scale = fitted_layout_x_scale(natural_width, target_width);
    let _ = cr.save();
    cr.translate(x, y);
    cr.scale(scale, 1.0);
    cr.move_to(0.0, 0.0);
    pangocairo::functions::show_layout(cr, layout);
    let _ = cr.restore();
}

fn fitted_layout_x_scale(natural_width: i32, target_width: f64) -> f64 {
    if natural_width <= 0 || target_width <= 0.0 {
        return 1.0;
    }
    target_width / f64::from(natural_width)
}

fn terminal_cell_pixel_size(
    width_logical: gtk::pango::Rectangle,
    height_logical: gtk::pango::Rectangle,
) -> (i32, i32) {
    (
        width_logical.width().max(1),
        height_logical
            .height()
            .saturating_add(TERMINAL_GLYPH_GUARD_PX)
            .max(1),
    )
}

fn renderer_cell_metrics_from_size(cell_size: (i32, i32)) -> RendererCellMetrics {
    RendererCellMetrics {
        width: f64::from(cell_size.0.max(1)),
        height: f64::from(cell_size.1.max(1)),
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

fn should_dim_pane(focused: bool, has_siblings: bool, model_focused: bool) -> bool {
    has_siblings && !focused && !model_focused
}

fn paint_unfocused_split_dim(cr: &gtk::cairo::Context, width: i32, height: i32) {
    cr.set_source_rgba(0.0, 0.0, 0.0, UNFOCUSED_SPLIT_DIM_ALPHA);
    cr.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
    let _ = cr.fill();
}

fn scrollback_indicator_geometry(
    top: usize,
    rows: usize,
    total: usize,
    widget_height: i32,
) -> Option<ScrollbackIndicatorGeometry> {
    if total == 0 || rows >= total || top.saturating_add(rows) >= total {
        return None;
    }
    if widget_height <= TERMINAL_PADDING_PX * 2 {
        return None;
    }
    let track_height = widget_height.saturating_sub(TERMINAL_PADDING_PX * 2).max(1);
    let min_height = (SCROLLBACK_INDICATOR_WIDTH_PX as i32).min(track_height);
    let height = ((track_height as f64 * rows as f64 / total as f64).round() as i32)
        .clamp(min_height, track_height);
    let max_offset = track_height.saturating_sub(height);
    let offset =
        ((track_height as f64 * top as f64 / total as f64).round() as i32).clamp(0, max_offset);
    Some(ScrollbackIndicatorGeometry {
        y: TERMINAL_PADDING_PX + offset,
        height,
    })
}

fn paint_scrollback_indicator(
    cr: &gtk::cairo::Context,
    width: i32,
    indicator: ScrollbackIndicatorGeometry,
) {
    let x = f64::from(width - TERMINAL_PADDING_PX) - SCROLLBACK_INDICATOR_WIDTH_PX;
    if x < 0.0 {
        return;
    }
    let y = f64::from(indicator.y);
    let height = f64::from(indicator.height);
    let radius = SCROLLBACK_INDICATOR_WIDTH_PX / 2.0;
    cr.set_source_rgba(1.0, 1.0, 1.0, SCROLLBACK_INDICATOR_ALPHA);
    cr.new_sub_path();
    cr.arc(x + radius, y + radius, radius, PI, PI * 2.0);
    cr.arc(x + radius, y + height - radius, radius, 0.0, PI);
    cr.close_path();
    let _ = cr.fill();
}

fn paint_visual_bell_border(cr: &gtk::cairo::Context, width: i32, height: i32) {
    if width <= VISUAL_BELL_BORDER_PX as i32 || height <= VISUAL_BELL_BORDER_PX as i32 {
        return;
    }
    let inset = VISUAL_BELL_BORDER_PX / 2.0;
    let border_width = f64::from(width) - VISUAL_BELL_BORDER_PX;
    let border_height = f64::from(height) - VISUAL_BELL_BORDER_PX;
    let _ = cr.save();
    cr.set_line_width(VISUAL_BELL_BORDER_PX);
    VISUAL_BELL_ACCENT.set_cairo_source(cr);
    cr.rectangle(inset, inset, border_width, border_height);
    let _ = cr.stroke();
    let _ = cr.restore();
}

#[cfg(test)]
mod tests {
    use super::*;
    use forktty_terminal::ghostty::core::{TerminalCell, TerminalRgb, TerminalRow};

    #[test]
    fn terminal_renderer_maps_default_colors_to_ansi_palette() {
        let config = config::AppConfig::default();
        let palette = RendererPalette::from_terminal_colors(&terminal_colors_for_config(&config));

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
    fn terminal_renderer_uses_configured_styled_fonts() {
        let config = config::AppConfig::default();
        let renderer = TerminalRenderer {
            palette: RendererPalette::from_terminal_colors(&terminal_colors_for_config(&config)),
            font: gtk::pango::FontDescription::from_string("Base Mono 12"),
            bold_font: Some(gtk::pango::FontDescription::from_string("ForkTTYBold 12")),
            italic_font: Some(gtk::pango::FontDescription::from_string("ForkTTYItalic 12")),
            bold_italic_font: Some(gtk::pango::FontDescription::from_string(
                "ForkTTYBoldItalic 12",
            )),
        };

        assert_eq!(
            renderer.font_for_style(true, false).family().as_deref(),
            Some("ForkTTYBold")
        );
        assert_eq!(
            renderer.font_for_style(false, true).family().as_deref(),
            Some("ForkTTYItalic")
        );
        assert_eq!(
            renderer.font_for_style(true, true).family().as_deref(),
            Some("ForkTTYBoldItalic")
        );
    }

    #[test]
    fn fitted_layout_x_scale_maps_text_width_to_cell_width() {
        assert_eq!(fitted_layout_x_scale(30, 45.0), 1.5);
        assert_eq!(fitted_layout_x_scale(0, 45.0), 1.0);
    }

    #[test]
    fn terminal_cell_metrics_reserve_bottom_pixel_for_glyph_edges() {
        let width_logical = gtk::pango::Rectangle::new(0, 0, 10, 23);
        let height_logical = gtk::pango::Rectangle::new(0, 0, 10, 23);

        assert_eq!(
            terminal_cell_pixel_size(width_logical, height_logical),
            (10, 24)
        );
    }

    #[test]
    fn terminal_cell_metrics_keep_normal_line_spacing_for_box_drawing() {
        let width_logical = gtk::pango::Rectangle::new(0, 0, 10, 24);
        let height_logical = gtk::pango::Rectangle::new(0, 0, 70, 24);

        assert_eq!(
            terminal_cell_pixel_size(width_logical, height_logical),
            (10, 25)
        );
    }

    #[test]
    fn renderer_cell_metrics_use_resize_cell_size() {
        let metrics = renderer_cell_metrics_from_size((11, 29));

        assert_eq!(metrics.width, 11.0);
        assert_eq!(metrics.height, 29.0);
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
            wrapped: false,
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
    fn terminal_renderer_ignores_legacy_config_palette() {
        let mut config = config::AppConfig::default();
        config.appearance.terminal_theme = config::TERMINAL_THEME_DRACULA.to_string();
        let renderer = TerminalRenderer::from_config_with_font(
            &config,
            gtk::pango::FontDescription::from_string("monospace 12"),
        );
        let default_cell = test_cell("x", None, None);

        assert_eq!(
            renderer.cell_background(&default_cell).to_string(),
            TerminalColors::forktty_dark().background
        );
        assert_eq!(
            renderer.cell_foreground(&default_cell).to_string(),
            TerminalColors::forktty_dark().foreground
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
                wrapped: false,
            },
        );

        let runs = renderer.text_runs_for_frame_row(&frame, &frame.rows[0], None);

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
            TerminalColors::forktty_dark().bold
        );
    }

    #[test]
    fn terminal_renderer_maps_bold_base_ansi_to_bright_ansi_when_configured() {
        let mut colors = TerminalColors::forktty_dark();
        colors.bold_is_bright = true;
        let renderer = TerminalRenderer {
            palette: RendererPalette::from_terminal_colors(&colors),
            font: gtk::pango::FontDescription::from_string("monospace 12"),
            bold_font: None,
            italic_font: None,
            bold_italic_font: None,
        };
        let mut red_cell = test_cell("x", Some(parse_rgb(&colors.ansi[1])), None);
        red_cell.bold = true;
        red_cell.foreground_palette = Some(1);
        let frame = test_frame(
            parse_rgb(&colors.foreground),
            parse_rgb(&colors.background),
            TerminalRow {
                cells: vec![red_cell],
                wrapped: false,
            },
        );

        assert_eq!(
            renderer
                .cell_foreground_for_frame(&frame, &frame.rows[0].cells[0])
                .to_string(),
            colors.ansi[9]
        );
    }

    #[test]
    fn terminal_renderer_underlines_hyperlink_cells() {
        let config = config::AppConfig::default();
        let renderer = TerminalRenderer::from_config_with_font(
            &config,
            gtk::pango::FontDescription::from_string("monospace 12"),
        );
        let mut link_cell = test_cell("x", None, None);
        link_cell.hyperlink = true;
        let row = TerminalRow {
            cells: vec![link_cell, test_cell("y", None, None)],
            wrapped: false,
        };

        let runs = renderer.text_runs_for_row(&row);

        assert_eq!(runs.len(), 2);
        assert!(runs[0].underline);
        assert!(!runs[1].underline);
    }

    #[test]
    fn terminal_renderer_underlines_hover_link_cols() {
        let config = config::AppConfig::default();
        let renderer = TerminalRenderer::from_config_with_font(
            &config,
            gtk::pango::FontDescription::from_string("monospace 12"),
        );
        let row = TerminalRow {
            cells: vec![
                test_cell("a", None, None),
                test_cell("b", None, None),
                test_cell("c", None, None),
                test_cell("d", None, None),
            ],
            wrapped: false,
        };
        // link_cols = Some((1, 3)): exclusive end, so cols 1-2 are underlined
        let runs = renderer.text_runs_for_row_with_defaults(
            &row,
            renderer.palette_defaults(),
            Some((1, 3)),
        );
        // col 0: no underline; cols 1-2: underline; col 3: no underline
        let underlined: Vec<bool> = runs
            .iter()
            .flat_map(|run| {
                let n = run.text.chars().count();
                std::iter::repeat_n(run.underline, n)
            })
            .collect();
        assert_eq!(underlined, vec![false, true, true, false]);
    }

    #[test]
    fn terminal_renderer_paints_wide_cell_background_across_cell_span() {
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
        let mut wide = test_cell("橋", None, Some(red));
        wide.width = TerminalCellWidth::Wide;
        let mut spacer = test_cell("", None, None);
        spacer.width = TerminalCellWidth::SpacerTail;
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
                cells: vec![wide, spacer],
                wrapped: false,
            },
        );

        let backgrounds = renderer.background_runs_for_frame_row(&frame, &frame.rows[0]);

        assert_eq!(backgrounds.len(), 1);
        assert_eq!(backgrounds[0].start_col, 0);
        assert_eq!(backgrounds[0].cell_span, 2);
        assert_eq!(
            backgrounds[0].background,
            RendererColor::from_terminal_rgb(red)
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
            wrapped: false,
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
            wrapped: false,
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
            wrapped: false,
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
                wrapped: false,
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

    #[test]
    fn terminal_renderer_preserves_non_block_cursor_shape() {
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
                cells: vec![test_cell("a", None, None)],
                wrapped: false,
            },
        )
        .with_cursor_style(
            0,
            0,
            forktty_terminal::ghostty::core::TerminalCursorStyle::Bar,
        );

        let overlay = renderer.cursor_overlay_for_frame(&frame).unwrap();

        assert_eq!(overlay.style, RendererCursorStyle::Bar);
        assert_eq!(overlay.cell_span, 1);
    }

    #[test]
    fn terminal_renderer_dims_only_unfocused_panes_with_siblings() {
        assert!(!should_dim_pane(true, false, false));
        assert!(!should_dim_pane(false, false, false));
        assert!(!should_dim_pane(true, true, false));
        assert!(should_dim_pane(false, true, false));
    }

    #[test]
    fn terminal_renderer_never_dims_the_model_focused_pane() {
        // The model-focused pane stays undimmed even while its search entry
        // (or the command palette, or a dialog) owns GTK focus.
        assert!(!should_dim_pane(false, true, true));
        assert!(!should_dim_pane(false, false, true));
        // A sibling that is neither GTK- nor model-focused still dims.
        assert!(should_dim_pane(false, true, false));
    }

    #[test]
    fn scrollback_indicator_geometry_hides_at_bottom_or_without_scrollback() {
        assert_eq!(scrollback_indicator_geometry(90, 10, 100, 116), None);
        assert_eq!(scrollback_indicator_geometry(0, 10, 10, 116), None);
    }

    #[test]
    fn scrollback_indicator_geometry_scales_with_viewport() {
        assert_eq!(
            scrollback_indicator_geometry(20, 10, 100, 116),
            Some(ScrollbackIndicatorGeometry { y: 27, height: 10 })
        );
    }

    #[test]
    fn scrollback_indicator_geometry_rounds_thumb_edges() {
        assert_eq!(
            scrollback_indicator_geometry(7, 33, 100, 115),
            Some(ScrollbackIndicatorGeometry { y: 13, height: 34 })
        );
    }

    #[test]
    fn scrollback_indicator_geometry_handles_tiny_widget_height() {
        assert_eq!(scrollback_indicator_geometry(0, 1, 10, 2), None);
    }

    #[test]
    fn terminal_renderer_places_wide_tail_cursor_over_wide_cell() {
        let config = config::AppConfig::default();
        let renderer = TerminalRenderer::from_config_with_font(
            &config,
            gtk::pango::FontDescription::from_string("monospace 12"),
        );
        let mut wide = test_cell("橋", None, None);
        wide.width = TerminalCellWidth::Wide;
        let mut spacer = test_cell("", None, None);
        spacer.width = TerminalCellWidth::SpacerTail;
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
                cells: vec![wide, spacer],
                wrapped: false,
            },
        )
        .with_wide_tail_cursor(1, 0);

        let overlay = renderer.cursor_overlay_for_frame(&frame).unwrap();

        assert_eq!(overlay.col, 0);
        assert_eq!(overlay.text, "橋");
        assert_eq!(overlay.cell_span, 2);
    }

    fn test_cell(
        text: &str,
        foreground: Option<TerminalRgb>,
        background: Option<TerminalRgb>,
    ) -> TerminalCell {
        TerminalCell {
            text: text.to_string(),
            foreground,
            foreground_palette: None,
            background,
            width: TerminalCellWidth::Narrow,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            inverse: false,
            invisible: false,
            hyperlink: false,
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
            palette: TerminalColors::forktty_dark()
                .ansi
                .map(|color| parse_rgb(&color)),
            cursor: None,
            rows: vec![row],
        }
    }

    fn parse_rgb(value: &str) -> TerminalRgb {
        let color = RendererColor::parse(value);
        TerminalRgb {
            red: color.red,
            green: color.green,
            blue: color.blue,
        }
    }

    trait TestFrameCursor {
        fn with_cursor(self, x: u16, y: u16) -> Self;
        fn with_cursor_style(
            self,
            x: u16,
            y: u16,
            style: forktty_terminal::ghostty::core::TerminalCursorStyle,
        ) -> Self;
        fn with_wide_tail_cursor(self, x: u16, y: u16) -> Self;
    }

    impl TestFrameCursor for TerminalFrame {
        fn with_cursor(mut self, x: u16, y: u16) -> Self {
            self.cursor = Some(forktty_terminal::ghostty::core::TerminalCursor {
                x,
                y,
                visible: true,
                at_wide_tail: false,
                style: forktty_terminal::ghostty::core::TerminalCursorStyle::Block,
            });
            self
        }

        fn with_cursor_style(
            mut self,
            x: u16,
            y: u16,
            style: forktty_terminal::ghostty::core::TerminalCursorStyle,
        ) -> Self {
            self.cursor = Some(forktty_terminal::ghostty::core::TerminalCursor {
                x,
                y,
                visible: true,
                at_wide_tail: false,
                style,
            });
            self
        }

        fn with_wide_tail_cursor(mut self, x: u16, y: u16) -> Self {
            self.cursor = Some(forktty_terminal::ghostty::core::TerminalCursor {
                x,
                y,
                visible: true,
                at_wide_tail: true,
                style: forktty_terminal::ghostty::core::TerminalCursorStyle::Block,
            });
            self
        }
    }
}
