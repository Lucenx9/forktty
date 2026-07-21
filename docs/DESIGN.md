# ForkTTY visual rules

Use these rules for GTK UI changes so the app stays quiet, dense, and native.

## Color

- Use one accent: `#e88745`.
- Avoid gradients, glow effects, accent ramps, and extra brand colors.
- Use dark surfaces literally: `#181818` for the deepest app surface and `#232323` for raised or pane-adjacent surfaces.
- Keep terminal theme colors owned by the terminal renderer and OSC/theme state.
- In `crates/forktty-ui-gtk/src/style.css`, use literal colors or `@named_color` only. Avoid CSS custom properties (`--x`, `var(--x)`) because GTK 4.14 drops them in AppImage builds.

## Layout

- Use a 6px spacing grid for app chrome, sidebars, menus, status rows, and dialogs.
- Prefer stock Adwaita widgets and spacing before custom CSS.
- Keep operational UI compact. Avoid marketing-page composition, oversized headings, nested cards, and decorative backgrounds.
- Use icons for toolbar-style actions when a stock or bundled icon exists.
- Keep cards only for repeated items, popovers, dialogs, and framed tools.
- Keep the workspace sidebar as a narrow Adwaita overlay so opening it does not
  reflow terminal panes. Workspace rows are flat navigation, and routine agent
  summaries stay hidden unless the workspace needs attention or reports an
  error or terminal-exit state.

## Motion

- Use the established durations, all ease-out: 90ms for CSS state transitions (hover, focus, color), 150ms for transient feedback (visual bell flash, scrollbar fade), and 110-180ms for structural motion (settings crossfade 110ms, pane header revealer 180ms).
- New animations pick one of these durations instead of introducing another.
- Avoid competing animation durations or attention-grabbing loops.
- Timers and animation callbacks must hold weak refs so closed panes can die.

## Text

- Use sentence case for UI strings.
- Keep labels terse and task-oriented.
- Avoid emoji in UI text.
- Avoid explanatory copy inside the app unless it is needed to complete a workflow.

## Terminal Pane Polish

- Terminal content padding is `TERMINAL_PADDING_PX = 6` in `terminal_geometry.rs`.
- Padding is a GTK-layer concern: the renderer fills the whole widget background, then offsets the grid.
- Input mapping, selection, mouse forwarding, and drawing must use the same renderer cell metrics.
- Balance padding remainder on both axes so the grid is centered when pixels do not divide evenly.
- Unfocused split dimming for current panes comes from Ghostty config (`unfocused-split-opacity` / `unfocused-split-fill`) when present; legacy `terminal_renderer.rs` dimming applies only to the old classic renderer code path.
- Dim only unfocused panes when the visible workspace has more than one terminal pane.
- In split layouts, mark only the focused pane with the warm header hairline.
  Keep agent lifecycle in that header and avoid duplicating it in a pane footer.
- Visual bell uses the accent color as a short 2px inner border. Do not add sound.
- The scrollback indicator is a minimal right-edge overlay; avoid permanent chrome.

## CSS Boundaries

- Keep `style.css` compatible with GTK 4.14.
- Do not add gtk/adw version features to access newer styling APIs.
- Prefer Adwaita states and classes over bespoke selectors.
- Avoid restyling unrelated widgets while touching one surface.
