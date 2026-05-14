# GTK UI polish + chrome redesign

**Date:** 2026-05-14
**Scope:** Polish + chrome redesign (Scope B). Visual language target: Zed/Ghostty — minimal, dense.
**Affected crates:** `forktty-ui-gtk` (primary), `forktty-core` (config additions for sidebar visibility).

---

## 1. Goals and non-goals

### Goals

- Eliminate visual noise that hides hierarchy or duplicates information: the purple window outline, the `VTE` badge, `ACTIVE`/`IDLE` text pills, duplicated CWD strings, the `Ready` filler label, the centered `ForkTTY` title.
- Give the chrome a real Zed/Ghostty-style minimal-dense feel: thinner titlebar, denser sidebar, pane header that disappears for single-pane workspaces, a real global status bar.
- Make the design system internally consistent so future styling is additive (real text hierarchy, no aliased semantic tokens, pane surfaces derived from `--ft-bg-*`).
- Add Ctrl+B sidebar toggle with persistence in `AppConfig`.

### Non-goals

- No git branch / git status in the status bar (requires plumbing in `forktty-core` outside this scope).
- No running-command or exit-code indicators.
- No redesign of command palette, worktree dialog, or notification panel internals (only token alignment).
- No light theme work. The app stays dark; tokens are restructured so light is possible later but not delivered now.
- No new keyboard shortcuts beyond Ctrl+B.
- No animation beyond fade transitions on color/opacity.

---

## 2. Section 1 — Design tokens

### Problem

`style.css:23-26` aliases `--ft-text-strong`, `--ft-text`, `--ft-text-muted`, `--ft-text-faint` all to `--window-fg-color` — there is no real text hierarchy by color. `style.css:36-42` aliases `--ft-warning-soft`, `--ft-error-soft`, `--ft-success-soft`, `--ft-info-soft` all to `--card-bg-color` — no semantic differentiation for tinted states. `style.css:15-17` hardcodes pane backgrounds (`#1c1d2a`, `#262838`, `#2f3146`).

### Change

In `:root`:

- **Text hierarchy** via `alpha()` over `--window-fg-color`:
  - `--ft-text-strong: var(--window-fg-color);` (100%)
  - `--ft-text: alpha(var(--window-fg-color), 0.85);`
  - `--ft-text-muted: alpha(var(--window-fg-color), 0.60);`
  - `--ft-text-faint: alpha(var(--window-fg-color), 0.40);`
- **Semantic soft tints** via `alpha()` over each semantic color:
  - `--ft-warning-soft: alpha(var(--warning-color), 0.14);`
  - `--ft-error-soft: alpha(var(--error-color), 0.14);`
  - `--ft-success-soft: alpha(var(--success-color), 0.14);`
  - `--ft-info-soft: alpha(var(--accent-color), 0.14);`
- **Pane surfaces** tokenized (no more hex hardcoded):
  - `--ft-pane-bg: shade(var(--ft-bg-1), 0.94);` — slightly deeper than window
  - `--ft-pane-header: shade(var(--ft-bg-1), 1.06);`
  - `--ft-pane-header-active: shade(var(--ft-bg-1), 1.12);`
- **Fallback if `shade()` / `alpha()` aren't supported by the deployed GTK CSS engine**: replace each token's value with an explicit hex picked to match the intended ratio against the current dark `--window-fg-color`/`--window-bg-color`. Computed once at implementation, not deferred to a runtime decision. The hex set below is the canonical fallback if the dynamic forms fail:
  - `--ft-pane-bg: #1b1c27;`
  - `--ft-pane-header: #23253250;` (i.e. existing-ish, slightly tinted)
  - `--ft-pane-header-active: #2c2e42;`
  - `--ft-text: rgba(255,255,255,0.85);` (and the muted/faint pair at 0.60 / 0.40)
  - `--ft-*-soft: rgba(<r,g,b of the semantic color>,0.14);`

### Verification

- Pane visible darker than terminal stage area.
- A muted label (e.g., workspace path in sidebar row) is visually distinguishable from the workspace name above it.
- Switching system accent color (GNOME Settings) propagates to pane headers and outline.

---

## 3. Section 2 — Header bar

### Problem

`gtk_app.rs:1468-1509` builds an `adw::HeaderBar` with `min-height: 50px` (style.css:74), default Adwaita title showing `ForkTTY` centered, 32px icon buttons with `padding: 0 6px`. Wasted vertical space; centered title is dead weight.

### Change

In `gtk_app.rs:build_ui`:

- Remove the default Adwaita title by calling `header.set_title_widget(Some(&workspace_title_widget))` where `workspace_title_widget` is a `gtk::Button` (flat, no border, drag-handle preserved) showing the active workspace name. Click handler: open the command palette with the query field pre-populated with a workspace-filter prefix (or, equivalently, scroll/focus the workspace section if the palette doesn't support text-prefix filtering — pick whichever is already supported by `show_command_palette` at `gtk_app.rs:2315`; do not invent a new palette mode). Tooltip: `Switch workspace (Ctrl+Shift+P)`.
- The workspace title widget exposes a `set_label(&str)` we call from `refresh_sidebar` (or a new sibling that fires on workspace switch) whenever the active workspace changes.
- Keep `pack_start`: `new_workspace`, `new_worktree`.
- Keep `pack_end`: `command_palette`, `notifications`, `settings`. Order unchanged.

In `style.css`:

- `.app-header { min-height: 38px; }` (down from 50px).
- `.header-action { min-width: 28px; min-height: 28px; padding: 0 4px; border-radius: 6px; }`.
- `.app-header { border-bottom: 1px solid var(--ft-border-subtle); box-shadow: none; }`.
- New class `.app-header-title` for the workspace title widget: muted text, 0.92em, weight 500, no background, hover background `var(--ft-bg-3)`, cursor pointer; when no workspace selected, hidden via `visibility: hidden` (still occupies layout for drag handle).

### Verification

- Window title says nothing (no "ForkTTY"). Workspace name visible in the center.
- Header bar measurably shorter than before.
- Click on workspace name opens command palette.
- Window drag still works on the centered area.

---

## 4. Section 3 — Sidebar

### Problem

`gtk_app.rs:1516-1559` builds a 260px sidebar with two stacked headers (`ForkTTY` brand + `VTE` mode pill at the top, `Ready` + keycap at the bottom). Workspace rows have a `X panes` pill regardless of pane count. No toggle.

### Change

#### 4.1 Width and structure (`gtk_app.rs`)

- `sidebar_shell.set_width_request(220)` (down from 260).
- Remove the `sidebar_footer` box entirely (`gtk_app.rs:1541-1555`) and the lines that append it. The `ready` label and `palette_hint` move to the new global status bar (Section 6). Drop their construction here.
- Sidebar header rebuilt:
  - Remove `sidebar_title` showing `ForkTTY`.
  - Remove `sidebar_mode` showing `VTE`.
  - New layout: small uppercase label `WORKSPACES` (class `.sidebar-section-label`) on the left, a flat `+` button (icon `tab-new-symbolic`, class `.sidebar-add`) on the right. The `+` button delegates to the same action as the titlebar `new_workspace` (`win.new-workspace` or whatever action name exists).

#### 4.2 Row density (`refresh_sidebar` in `gtk_app.rs:1983` and CSS)

- Row padding `8/12 → 6/10`. Row min-height target ~36px.
- Remove the `workspace-pill` showing `X pane(s)`. If pane count > 1, append a small muted numeric label (class `.workspace-count`) inline after the workspace name (e.g. `main  2`). For 1 pane, render nothing.
- Active row: keep `inset 2px 0 0 var(--ft-accent)` left bar, keep subtle bg tint.
- `needs-attention`: keep current warning treatment but apply to the same border location.

#### 4.3 Toggle Ctrl+B (`gtk_app.rs` + `forktty-core` config)

- Add field `pub sidebar_visible: bool` (default `true`) to `forktty-core::config::AppearanceConfig` at `crates/forktty-core/src/config.rs:47`, alongside the existing `sidebar_position`. Use the same `#[serde(default = "...")]` pattern as the surrounding fields (introduce `default_sidebar_visible() -> bool { true }`). Update `Default for AppearanceConfig` at `config.rs:81` to set the field.
- Persistence path already exists: `pub fn save_config(config: &AppConfig)` at `config.rs:116` is the canonical write path. The toggle action reads the current `AppConfig`, mutates `appearance.sidebar_visible`, and calls `save_config(...)`. No new helper required.
- Add an action consistent with the existing `install_actions` pattern (`gtk_app.rs:1933`), keybind `<Primary>b`. The action calls `sidebar_shell.set_visible(!sidebar_shell.is_visible())` then persists via `save_config`.
- On startup, `build_ui` reads `app_config.appearance.sidebar_visible` and calls `sidebar_shell.set_visible(...)` accordingly.

#### 4.4 CSS

- `.sidebar-shell { border-right: 1px solid var(--ft-border-subtle); }`.
- `.sidebar-header { min-height: 32px; padding: 0 10px; }`.
- `.sidebar-section-label { font-size: 0.72em; font-weight: 700; letter-spacing: 0.06em; text-transform: uppercase; color: var(--ft-text-faint); }`.
- `.sidebar-add { min-width: 22px; min-height: 22px; padding: 0; }`.
- `.navigation-sidebar row { margin: 1px 0; }`.
- `.workspace-card { padding: 6px 10px; }`.
- `.workspace-count { color: var(--ft-text-muted); font-size: 0.78em; font-weight: 600; margin-left: 6px; }`.

### Verification

- Sidebar visibly narrower; rows denser.
- Single-workspace state: no `1 pane` pill anywhere.
- Two-pane workspace: small `2` next to name.
- Ctrl+B hides the sidebar; pressing again restores. Restart the app: state persists.
- No `ForkTTY` label or `VTE` badge anywhere in the sidebar.

---

## 5. Section 4 — Pane chrome

### Problem

`gtk_app.rs:446-529` builds a 32px pane header with grip `⋮`, title, CWD (often identical text to title), `Active/Idle/Unread` text pill, and always-visible split/close buttons. With four narrow panes the header truncates badly and reads as noise.

### Change

#### 5.1 Hide header on single-pane workspaces

- Extend `PaneChrome` (`gtk_app.rs:154`) with a stored reference to the header `gtk::Box` so callers can toggle its visibility. Construction of the box already happens in `build_pane_chrome`; just store it in the struct alongside `pane`/`title`/`cwd`.
- Trigger site: `rebuild_layout` at `gtk_app.rs:257`. After constructing `pane_tree` (line 272) and before grabbing focus, compute `let single = collect_leaves(&pane_tree).len() == 1;` and iterate `self.chromes.values()`, calling `chrome.header.set_visible(!single)` on each. This re-runs on every split/close because both end up calling `rebuild_layout`.
- The accent border-top on `.terminal-pane` is the only active-state cue when the header is hidden.

#### 5.2 Header structure (`build_pane_chrome`)

- Remove the grip `gtk::Label::new(Some("⋮"))` and its `pane.append`. Drop the `pane-grip` CSS class entirely.
- Remove `state_label` (`gtk::Label::new(Some("Idle"))`) and all its toggles in `update_pane_chrome`. The pill is gone.
- Keep `title` and `cwd` labels but change rendering:
  - Title on the left.
  - CWD on the right, **only if `cwd_text != title.label()` after both are computed**. Otherwise hide the CWD label (`cwd.set_visible(false)`). This kills the visible duplicate seen in the screenshot.
- Add an attention indicator as the first child of the header `gtk::Box`, before the title: a `gtk::Box` (empty, class `.pane-attention-dot`), 6×6px, rendered as a filled circle via CSS (`border-radius: 50%; background: var(--ft-warning);`). Hidden by default; shown via `set_visible(true)` when `surface.unread || surface.needs_attention`. Using a CSS-sized box instead of a Pango bullet avoids sub-10px glyph rendering inconsistencies.
- Actions container (`actions`) wired to a `gtk::EventControllerMotion` on the header `gtk::Box`. On enter, `actions.add_css_class("revealed")`; on leave, `remove_css_class("revealed")`. Default state: `.terminal-pane-actions { opacity: 0; pointer-events: none; }` and `.terminal-pane-actions.revealed { opacity: 1; pointer-events: auto; }`.
- Header height: CSS `.terminal-pane-header { min-height: 22px; padding: 0 8px; }` (from 32px / `0 12px`).
- Button size: `.terminal-pane-action { min-width: 20px; min-height: 20px; padding: 0; }`.

#### 5.3 update_pane_chrome (`gtk_app.rs:553`)

- Remove all `chrome.state.set_label(...)` and `chrome.state.add/remove_css_class(...)` lines.
- After setting title and cwd, compare and toggle CWD visibility as described above.
- Keep the `.active` / `.needs-attention` class toggles on `chrome.pane`.
- Toggle the attention dot visibility based on the same condition currently used for `needs-attention`.

#### 5.4 CSS

- `.terminal-pane-header { min-height: 22px; padding: 0 8px; }`.
- `.terminal-pane-title { font-size: 0.82em; color: var(--ft-text-muted); }`.
- `.terminal-pane.active .terminal-pane-title { color: var(--ft-text); font-weight: 600; }`.
- `.terminal-pane-cwd { font-size: 0.78em; color: var(--ft-text-faint); }`.
- `.terminal-pane-actions { opacity: 0; transition: opacity 90ms ease; pointer-events: none; }`.
- `.terminal-pane-actions.revealed { opacity: 1; pointer-events: auto; }`.
- `.pane-attention-dot { min-width: 6px; min-height: 6px; border-radius: 50%; background: var(--ft-warning); margin-right: 4px; }`.
- Remove `.pane-grip`, `.terminal-pane-state` (and its `.active`/`.unread` variants) — dead code after this section lands.

### Verification

- Open a single-pane workspace: no header above the terminal at all.
- Split horizontally: both panes show a thin header.
- The header reads `Terminal · ~/forktty` only when title and CWD differ; when shell sets title to CWD, only one of the two strings shows.
- Hover the header: split/copy/close fade in within ~90ms; leave: fade out.
- No `Active`/`Idle`/`Unread` text anywhere.
- An attention pane shows a small warning dot before the title; on focus, dot disappears.

---

## 6. Section 5 — Global status bar

### Problem

The current `sidebar_footer` (`gtk_app.rs:1541-1555`) lives inside the sidebar shell. When the sidebar is hidden (Section 4.3), all status info disappears. The `Ready` label is filler.

### Change

#### 6.1 Structure (`gtk_app.rs:build_ui`)

- After the `paned` (`gtk_app.rs:1566`), create a global vertical box `app_root_box` that wraps `header` + `paned` + new `status_bar`. The window content child becomes `app_root_box`.
- `status_bar` is a `gtk::Box::new(gtk::Orientation::Horizontal, 8)` with class `.app-status-bar`.
- Left: a flat `gtk::Button` (class `.status-location`) showing `<workspace> · <cwd-compact>`. Click handler: identical to the titlebar workspace title widget (Section 3) — both call the same helper that opens the palette in workspace-switch mode. Tooltip: `Switch workspace (Ctrl+Shift+P)`.
- Spacer: `gtk::Box` with `hexpand=true`.
- Right: keycap `gtk::Label::new(Some("Ctrl+Shift+P"))` with class `.keycap` (reusing the existing rule at `style.css:364-374`). Tooltip: `Open the Command Palette`.
- Update path: wherever active surface/workspace changes (the sidebar refresh + pane focus paths), call `status_location_set_label(...)`. The label is the same string used in the titlebar workspace name on the left side, plus ` · ` + `compact_path(&active_surface.cwd)` on the right side.

#### 6.2 CSS

- `.app-status-bar { min-height: 22px; padding: 0 10px; background: var(--ft-bg-2); border-top: 1px solid var(--ft-border-subtle); color: var(--ft-text-muted); font-size: 0.78em; }`.
- `.status-location { padding: 0 4px; background: transparent; border: 0; color: var(--ft-text-muted); font-size: 0.78em; }`.
- `.status-location:hover { background: var(--ft-bg-3); color: var(--ft-text); }`.
- The existing `.keycap` rule already produces the right look at this size; if it feels too large, tighten to `font-size: 0.72em; padding: 1px 5px;` for status-bar context only via a specialization `.app-status-bar .keycap`.

### Verification

- Status bar visible at the bottom of the window, full width.
- Hiding the sidebar (Ctrl+B) does not hide the status bar.
- Left text updates when switching workspace or focused pane.
- Click on left text opens command palette.

---

## 7. Section 6 — Outline bug + microinteractions

### Problem

`style.css:49-58` applies `outline-color` to `*` and `outline: 2px solid` to `*:focus-visible`. The `window` and `.app-root` widgets, plus the titlebar drag handle, can receive focus-visible state and render a purple ring around the entire window — the artifact visible in the screenshots.

### Change

Replace the universal selectors:

```css
button:focus-visible,
listview row:focus-visible,
.navigation-sidebar row:focus-visible,
entry:focus-visible,
.ft-menu-item:focus-visible,
.terminal-pane-action:focus-visible,
.status-location:focus-visible {
  outline: 2px solid var(--ft-accent);
  outline-offset: 2px;
}
```

Delete:

```css
* {
  outline-color: var(--ft-accent);
  outline-offset: 2px;
}

*:focus-visible {
  outline-width: 2px;
  outline-style: solid;
  outline-color: var(--ft-accent);
}
```

Microinteraction tightening:

- Grep `style.css` for `120ms` first. For every match whose `transition` property list contains only `color`, `background-color`, `border-color`, and/or `opacity`, replace `120ms` with `90ms`. If any rule transitions a non-color property (e.g. `margin`, `transform`), leave it untouched — but a grep at spec time shows the only properties currently animated are the four above, so in practice this is a clean global replace.
- `cursor: pointer;` added on `.workspace-card`, `.app-header-title`, `.status-location`, palette rows. GTK supports `cursor: pointer` in CSS.
- No transforms, no scales, no slide-in. Conservative: only color/opacity fades.

### Verification

- Click empty area of titlebar / window background / sidebar shell — no purple outline appears.
- Tab-focus a sidebar workspace row — outline appears around the row only.
- Hover over workspace card — cursor turns into pointer.

---

## 8. File-level change inventory

| File | Change |
|---|---|
| `crates/forktty-ui-gtk/src/style.css` | Token redefinitions; titlebar/sidebar/pane/status-bar rules; outline fix; 120→90ms transitions; dead rule cleanup (`pane-grip`, `terminal-pane-state*`, old `workspace-pill`, old `sidebar-ready`/`sidebar-footer`). |
| `crates/forktty-ui-gtk/src/gtk_app.rs` | `build_ui`: titlebar workspace widget, sidebar header rebuild, removal of sidebar footer, global status bar wrapping. `build_pane_chrome`: drop grip, drop state label, hover controller for actions, attention dot. `update_pane_chrome`: drop state label updates, conditional CWD visibility, attention dot toggle. Layout/refresh: hide pane header when leaf-count == 1. New `win.toggle-sidebar` action + `<Primary>b` keybind. New `set_label` callbacks for titlebar and status-bar location. |
| `crates/forktty-core/src/config.rs` (or equivalent) | Add `appearance.sidebar_visible: bool` (default `true`). Add or use existing atomic write helper to persist on toggle. |
| `crates/forktty-core/src/model.rs` | Untouched by this spec unless required for status-bar location plumbing — verify at implementation. The existing uncommitted diff (model.rs +246) is unrelated and should not be folded into this work. |

---

## 9. Out-of-scope follow-ups

Recorded here so they don't get rediscovered later as if they were new:

- Git branch / dirty marker in status bar.
- Running-command and exit-code indicator on active pane.
- Notification unread counter badge.
- Light theme pass (tokens now permit it, but no light QA in this iteration).
- Command palette / worktree dialog / notification panel layout redesign.
- Tabbar-like alternative to sidebar.

---

## 10. Risk and rollback

- All changes are isolated to `forktty-ui-gtk` and a single config field in `forktty-core`. No protocol or socket changes.
- If `shade()` / `alpha()` CSS functions are not supported in the deployed GTK version, fall back to the canonical hex/rgba set listed in Section 2 (no runtime decision).
- Rollback: revert the commit(s) on this branch. No data migration required for the new config field — older builds simply ignore the unknown key.
