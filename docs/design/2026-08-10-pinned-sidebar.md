# Feature quality brief: pinned workspace sidebar

**Status:** Implemented

**Owner:** ForkTTY maintainers

**Source:** User request, 2026-08-10

**Related plan:** N/A

## Summary

Users can keep the workspace sidebar in its existing overlay presentation or
pin it beside the terminal content. The chosen presentation persists across
restarts and remains independent from whether the sidebar is currently shown.

## Goals and non-goals

### Goals

- Preserve the current overlay sidebar as the default presentation.
- Provide an accessible control in the sidebar header for pinning or unpinning
  the sidebar without moving workspace content between views.
- Persist the pinned presentation and apply it immediately, including after a
  settings reset.

### Non-goals

- Changing the sidebar width, position choices, workspace rows, or keyboard
  shortcuts.
- Adding responsive breakpoints or swipe gestures.
- Changing pane layout or terminal session persistence.

## Scope and approvals

**In scope:** Appearance config, GTK workbench layout and sidebar header,
config/GTK regression tests, product contract, changelog, README, and matching
public-site workspace documentation.

**Out of scope:** Socket methods, session format, terminal rendering, worktree
behavior, dependencies, packaging, and release automation.

**Must not change:** `Ctrl+B`/F9 behavior, sidebar left/right placement,
GTK 4.14 CSS compatibility, browser feature gating, and terminal focus/layout
state.

**Approval required:** N/A; the user explicitly requested implementation and a
pull request.

## Sources and assumptions

- **External behavior source:** The official libadwaita 1.8
  [`AdwOverlaySplitView:pin-sidebar`](https://gnome.pages.gitlab.gnome.org/libadwaita/doc/1.8/property.OverlaySplitView.pin-sidebar.html)
  and
  [`AdwOverlaySplitView:collapsed`](https://gnome.pages.gitlab.gnome.org/libadwaita/doc/1.8/property.OverlaySplitView.collapsed.html)
  documentation. `collapsed` selects overlay versus side-by-side presentation;
  `pin-sidebar` prevents automatic visibility changes while that presentation
  changes.
- **Assumptions:** "Fixed" means a side-by-side sidebar that occupies layout
  width rather than covering the terminal. The pin choice is global appearance
  state, matching existing sidebar position and visibility preferences.
- **Open questions:** None.

## Requirements

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| REQ-001 | Existing configs and the default config use the current collapsed overlay presentation. | Config default test and GTK overlay regression test. |
| REQ-002 | Activating the pin control shows the sidebar beside terminal content without changing its current shown/hidden state; deactivating it restores overlay presentation. | GTK presentation transition test. |
| REQ-003 | The pin control is keyboard/focus accessible, exposes clear pin/unpin text, reflects live config/reset changes, and persists the chosen state across restarts. | GTK source/UI test plus config round-trip test. |
| REQ-004 | F9/Ctrl+B and left/right sidebar placement behave identically in overlay and pinned presentations. | Existing toggle assertions plus extended GTK regression test. |

## Acceptance scenarios

### SCN-001: Pin the visible sidebar

- **Given:** The sidebar is visible in the default overlay presentation.
- **When:** The user activates the pin button.
- **Then:** The sidebar remains visible, moves beside the terminal, and the
  pinned choice is saved.
- **Covers:** REQ-002, REQ-003

### SCN-002: Restore overlay presentation

- **Given:** The sidebar is pinned and visible.
- **When:** The user deactivates the pin button.
- **Then:** The sidebar remains visible and overlays the terminal again.
- **Covers:** REQ-002

### SCN-003: Toggle visibility while pinned

- **Given:** The sidebar is pinned.
- **When:** The user presses F9 or Ctrl+B twice.
- **Then:** The sidebar hides and shows without changing the pinned choice.
- **Covers:** REQ-004

### SCN-004: Reset appearance state

- **Given:** The sidebar is pinned.
- **When:** The user resets settings to defaults.
- **Then:** The workbench and pin control both return to overlay presentation.
- **Covers:** REQ-001, REQ-003

## Failure, recovery, and edge cases

| ID | Trigger | Required behavior | Recovery or rollback | Covers |
| --- | --- | --- | --- | --- |
| EDGE-001 | Existing config omits the new key. | Load successfully in overlay mode. | The next config save writes the default value. | REQ-001 |
| EDGE-002 | Sidebar is hidden when pin state changes through live config application. | Preserve hidden state while changing presentation. | Showing it later uses the selected presentation. | REQ-002, REQ-004 |
| EDGE-003 | Background config persistence fails after a user click. | Keep the immediate in-process layout change and log the existing-style error. | A later successful toggle or restart from the last saved config restores a durable state. | REQ-003 |

The UI interaction is the in-process commit point. Disk persistence remains the
existing asynchronous read-modify-write path used by sidebar visibility, so it
does not block GTK input.

## Security and privacy

- **Trust boundary impact:** None.
- **Input and size limits:** Unchanged; the new value is a boolean.
- **Data exposure/storage:** One local appearance boolean in ForkTTY's existing
  config file.
- **Command execution:** None.

## Architecture and release impact

- **Owning crate/module:** `forktty-core::config` owns persistence;
  `forktty-ui-gtk::gtk_app::app` owns workbench presentation and the control.
- **Dependency direction:** Unchanged; GTK reads the core config without adding
  lower-crate GUI dependencies.
- **Feature combinations:** Both `gtk-ghostty` and `browser` builds must remain
  green.
- **Session/config/socket compatibility:** Additive config key with a false
  default; no session or socket changes.
- **Packaging/runtime:** No new dependency or runtime asset; the button uses the
  standard Adwaita `view-pin-symbolic` icon.
- **Public site/docs:** Update `README.md`, `SPEC.md`, `CHANGELOG.md`, and the
  site's workspace docs plus LLM context.

## Implementation outline

1. **Slice 1:** Add core config default and round-trip tests, then add the
   backward-compatible `sidebar_pinned` field.
2. **Slice 2:** Extend the GTK overlay seam tests for overlay/pinned transitions,
   then implement presentation application and the accessible header toggle.
3. **Slice 3:** Update contracts and public docs, run both feature combinations,
   and review the fixed diff against `origin/main`.

## Requirement traceability

| Requirement | Planned test/command | Contract/docs impact | Final evidence |
| --- | --- | --- | --- |
| REQ-001 | `cargo test -p forktty-core sidebar_pinned` and GTK overlay test | `SPEC.md`, README, changelog, site | `sidebar_pinned_defaults_to_false_when_missing`, `sidebar_pinned_round_trips_through_save_and_load`, and `workbench_sidebar_overlays_terminal_content_and_preserves_configured_side` pass. |
| REQ-002 | GTK overlay presentation transition test | `SPEC.md`, README, site | `workbench_sidebar_switches_between_overlay_and_pinned_presentations` passes for visible and hidden transitions in both directions. |
| REQ-003 | Config round-trip and GTK control source/UI assertions | `SPEC.md`, changelog | `sidebar_pin_button_reflects_the_selected_presentation` and `applied_sidebar_config_syncs_layout_visibility_and_pin_control` pass; the latter exercises both live pinned config and default reset state. |
| REQ-004 | GTK overlay/toggle regression tests | README shortcut text remains stable | The workbench tests exercise right-side pinned layout and `toggle_sidebar_visibility` twice without changing pin state; source assertions retain F9/Ctrl+B accelerators. |

## Pre-implementation consistency review

- [x] Every requirement is unambiguous and objectively verifiable.
- [x] Primary, alternate, failure, recovery, and relevant non-functional
      scenarios are covered or explicitly excluded.
- [x] Every requirement maps to a planned task and acceptance seam.
- [x] The design follows ForkTTY's crate boundaries, critical constraints, and
      non-goals.
- [x] External behavior claims cite current primary sources.

**Findings:** None. The design explicitly maps the user's fixed-sidebar request
to libadwaita's side-by-side presentation instead of relying on the similarly
named `pin-sidebar` property alone.

## Post-implementation convergence review

| Finding | Classification | Requirement/source | Evidence | Resolution |
| --- | --- | --- | --- | --- |
| Initial tests changed visibility directly and checked only the left pinned position. | Evidence gap | REQ-004, spec review | First review of `workbench_sidebar_switches_between_overlay_and_pinned_presentations`. | Extracted the production visibility transition, exercised it in pinned mode, asserted pin/layout stability, and covered the right edge plus accelerator mapping. |
| Reset/live config synchronization was represented only by a source assertion. | Evidence gap | REQ-003, spec review | First review of the Settings apply path. | Extracted `apply_sidebar_config` and added a GTK test covering pinned live config followed by default reset state. |
| The accessible name changed with the toggle action even though assistive technology also announces checked state. | Implementation defect | REQ-003, UX review | `sync_sidebar_pin_button` used Pin/Unpin as both tooltip and accessible name. | Kept the tooltip action-oriented and changed the accessible name to the stable `Keep sidebar beside terminal`. |
| Manual release QA required overlay behavior unconditionally. | Documentation drift | REQ-002, REQ-004, UX review | `docs/release-qa.md` had no pinned scenario. | Split QA expectations across unpinned overlay and pinned side-by-side behavior, including visibility independence and restart persistence. |
| Reviewers looked for the site sync in a checkout containing unrelated user work. | Review-environment mismatch | Public-site alignment rule | Isolated site worktree commit `dd0f006`; 41 site tests and production build pass. | Kept the user's original checkout untouched and prepared the required site docs/LLM-context PR from `/home/simone/forktty-site-pinned-sidebar`. |

**Fixed review point:** `origin/main`

**Final verdict:** Ready
