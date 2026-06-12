pub(super) const TERMINAL_PADDING_PX: i32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TerminalGridGeometry {
    pub(super) origin_x: i32,
    pub(super) origin_y: i32,
    pub(super) grid_width: i32,
    pub(super) grid_height: i32,
}

pub(super) fn terminal_grid_cells_for_allocation(
    width_px: i32,
    height_px: i32,
    cell_width_px: i32,
    cell_height_px: i32,
) -> (u16, u16) {
    (
        pixel_cells(terminal_content_pixels(width_px), cell_width_px),
        pixel_cells(terminal_content_pixels(height_px), cell_height_px),
    )
}

pub(super) fn terminal_content_pixels(pixels: i32) -> i32 {
    pixels.saturating_sub(TERMINAL_PADDING_PX * 2).max(1)
}

pub(super) fn terminal_grid_geometry(
    width_px: i32,
    height_px: i32,
    cols: u16,
    rows: u16,
    cell_width_px: i32,
    cell_height_px: i32,
) -> TerminalGridGeometry {
    let cell_width_px = cell_width_px.max(1);
    let cell_height_px = cell_height_px.max(1);
    let grid_width = i32::from(cols.max(1)) * cell_width_px;
    let grid_height = i32::from(rows.max(1)) * cell_height_px;
    let extra_width =
        balanced_remainder(terminal_content_pixels(width_px), grid_width, cell_width_px);
    let extra_height = balanced_remainder(
        terminal_content_pixels(height_px),
        grid_height,
        cell_height_px,
    );
    TerminalGridGeometry {
        origin_x: TERMINAL_PADDING_PX + extra_width / 2,
        origin_y: TERMINAL_PADDING_PX + extra_height / 2,
        grid_width,
        grid_height,
    }
}

pub(super) fn padded_cell_for_position(
    geometry: &TerminalGridGeometry,
    x: f64,
    y: f64,
    cell_width_px: i32,
    cell_height_px: i32,
) -> (usize, usize) {
    (
        padded_axis_cell(x, geometry.origin_x, cell_width_px),
        padded_axis_cell(y, geometry.origin_y, cell_height_px),
    )
}

/// [`padded_cell_for_position`] clamped to the grid bounds: a pointer in the
/// trailing (bottom/right) padding maps to the last cell instead of one past
/// it. For point queries (word/line click selection, link resolution) only —
/// drag extension stays on the unclamped mapping, where exceeding the last
/// row is what extends the selection to end-of-line.
pub(super) fn padded_cell_for_position_clamped(
    geometry: &TerminalGridGeometry,
    x: f64,
    y: f64,
    cell_width_px: i32,
    cell_height_px: i32,
    cols: u16,
    rows: u16,
) -> (usize, usize) {
    let (col, row) = padded_cell_for_position(geometry, x, y, cell_width_px, cell_height_px);
    (
        col.min(usize::from(cols.saturating_sub(1))),
        row.min(usize::from(rows.saturating_sub(1))),
    )
}

pub(super) fn padded_mouse_position(geometry: &TerminalGridGeometry, x: f64, y: f64) -> (f64, f64) {
    (
        padded_mouse_axis_position(x, geometry.origin_x, geometry.grid_width),
        padded_mouse_axis_position(y, geometry.origin_y, geometry.grid_height),
    )
}

fn padded_axis_cell(position: f64, origin: i32, cell_pixels: i32) -> usize {
    ((position - f64::from(origin)).max(0.0) / f64::from(cell_pixels.max(1))) as usize
}

fn padded_mouse_axis_position(position: f64, origin: i32, grid_pixels: i32) -> f64 {
    let max_position = f64::from(grid_pixels.saturating_sub(1).max(0));
    (position - f64::from(origin)).clamp(0.0, max_position)
}

fn pixel_cells(pixels: i32, cell_pixels: i32) -> u16 {
    let cell_pixels = cell_pixels.max(1);
    ((pixels.max(1) / cell_pixels).max(1)).min(i32::from(u16::MAX)) as u16
}

fn balanced_remainder(content_pixels: i32, grid_pixels: i32, cell_pixels: i32) -> i32 {
    content_pixels
        .saturating_sub(grid_pixels)
        .rem_euclid(cell_pixels.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_grid_geometry_keeps_uniform_padding_and_splits_remainder() {
        let geometry = terminal_grid_geometry(119, 77, 10, 3, 10, 20);

        assert_eq!(
            geometry,
            TerminalGridGeometry {
                origin_x: 9,
                origin_y: 8,
                grid_width: 100,
                grid_height: 60,
            }
        );
    }

    #[test]
    fn grid_cells_for_allocation_uses_padded_content_area() {
        assert_eq!(
            terminal_grid_cells_for_allocation(800, 480, 10, 20),
            (78, 23)
        );
        assert_eq!(terminal_grid_cells_for_allocation(10, 10, 10, 20), (1, 1));
    }

    #[test]
    fn grid_geometry_does_not_center_stale_terminal_size() {
        let geometry = terminal_grid_geometry(700, 480, 80, 24, 7, 18);

        assert_eq!(geometry.origin_x, 7);
        assert_eq!(geometry.origin_y, 6);
    }

    #[test]
    fn padded_pixel_cell_mapping_subtracts_grid_origin() {
        let geometry = terminal_grid_geometry(119, 77, 10, 3, 10, 20);

        assert_eq!(
            padded_cell_for_position(&geometry, 8.0, 7.0, 10, 20),
            (0, 0)
        );
        assert_eq!(
            padded_cell_for_position(&geometry, 18.9, 27.9, 10, 20),
            (0, 0)
        );
        assert_eq!(
            padded_cell_for_position(&geometry, 19.0, 28.0, 10, 20),
            (1, 1)
        );
    }

    #[test]
    fn clamped_cell_mapping_pins_trailing_padding_to_the_last_cell() {
        // 10x3 grid: origin (9, 8), 100x60 px, so the last cell ends at
        // x = 109, y = 68 and the trailing gutter starts past that.
        let geometry = terminal_grid_geometry(119, 77, 10, 3, 10, 20);

        // A pointer a few px into the trailing padding — and one far past
        // the widget — both resolve to the last cell.
        assert_eq!(
            padded_cell_for_position_clamped(&geometry, 112.0, 70.0, 10, 20, 10, 3),
            (9, 2)
        );
        assert_eq!(
            padded_cell_for_position_clamped(&geometry, 500.0, 500.0, 10, 20, 10, 3),
            (9, 2)
        );
        // Interior pointers are unchanged.
        assert_eq!(
            padded_cell_for_position_clamped(&geometry, 19.0, 28.0, 10, 20, 10, 3),
            (1, 1)
        );
    }

    #[test]
    fn clamped_cell_mapping_is_safe_on_degenerate_grids() {
        let geometry = terminal_grid_geometry(119, 77, 10, 3, 10, 20);

        assert_eq!(
            padded_cell_for_position_clamped(&geometry, 500.0, 500.0, 10, 20, 0, 0),
            (0, 0)
        );
    }

    #[test]
    fn unclamped_cell_mapping_exceeds_the_grid_for_drag_extension() {
        // Drag-select relies on the bottom/right gutter mapping past the
        // last cell so dragging below the pane extends the selection to
        // end-of-line on the last row; this must stay unclamped.
        let geometry = terminal_grid_geometry(119, 77, 10, 3, 10, 20);

        assert_eq!(
            padded_cell_for_position(&geometry, 112.0, 70.0, 10, 20),
            (10, 3)
        );
    }

    #[test]
    fn padded_mouse_position_uses_grid_relative_pixels() {
        let geometry = terminal_grid_geometry(119, 77, 10, 3, 10, 20);

        assert_eq!(padded_mouse_position(&geometry, 7.0, 6.0), (0.0, 0.0));
        assert_eq!(padded_mouse_position(&geometry, 19.5, 30.0), (10.5, 22.0));
        assert_eq!(padded_mouse_position(&geometry, 200.0, 200.0), (99.0, 59.0));
    }
}
