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
- In the titlebar, the ForkTTY logo is the sole main-menu trigger. Keep it next
  to sidebar/workspace navigation instead of duplicating it with a static
  wordmark, hamburger, or separator.
- Keep the titlebar as the only permanent global bar. Do not add a bottom status
  bar for workspace, pane-count, or shortcut information already available in
  the titlebar, pane chrome, sidebar, tooltips, or command palette.
- Group titlebar tools and window controls with spacing, not decorative rules.
- Keep the workspace sidebar narrow and default it to an Adwaita overlay so
  opening it does not reflow terminal panes. Its header may pin the same sidebar
  beside the workbench when the user wants a persistent navigation surface.
  Workspace rows are flat navigation, and routine agent summaries stay hidden
  unless the workspace needs attention or reports an error or terminal-exit
  state. Model refreshes reuse unchanged rows instead of rebuilding the full
  list, preserving unaffected row identity and interaction.
- Keep Settings compact and goal-oriented: core pages precede optional
  integrations under General, Integrations, and System navigation. Preference
  groups use one subtle raised surface; navigation headings stay sentence case.
- Treat external configuration as a trust boundary. Before Settings writes an
  integration, state what ForkTTY owns, require confirmation, and provide a
  nearby removal path that preserves unrelated configuration.
- Keep the Worktree manager task-first: show the source workspace and path as a
  compact context row, keep the selected operation in one segmented control,
  and give the target field a persistent mode-specific label. Helper copy must
  explain consequences instead of repeating the placeholder; removal must say
  that the git branch remains intact.

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
- Give inactive panes that need attention a stronger two-pixel warm hairline
  and dot; focused panes retain the single-pixel focus treatment.
- Keep split-pane headers at 22 px, on the recessed stage surface. Drag cues stay
  faint until hover or focus, and pane actions remain hidden until interaction.
- Split dividers use a seven-pixel pointer target with a one-pixel idle line;
  hover and drag may tint the target without making the resting divider heavy.
- Keep pane-action hover fills inset from both header edges so they never cover
  the focused-pane hairline or read as a full-height flash during pointer travel.
- Visual bell uses the accent color as a short 2px inner border. Do not add sound.
- The scrollback indicator is a minimal right-edge overlay; avoid permanent chrome.

## Notifications

- Lead targeted notification rows with workspace and path context before title
  and body so the destination is visible before an action is taken.
- Preserve the panel's scroll position when visible notification content is
  reconciled; background refreshes must not return the user to the top.

## CSS Boundaries

- Keep `style.css` compatible with GTK 4.14.
- Do not add gtk/adw version features to access newer styling APIs.
- Prefer Adwaita states and classes over bespoke selectors.
- Avoid restyling unrelated widgets while touching one surface.
