# Feature quality brief: visual and functional QA fixes

**Status:** Implemented

**Owner:** ForkTTY maintainers

**Source:** User-requested whole-app visual and functional QA, 2026-08-11

**Related plan:** N/A

## Summary

Exercise ForkTTY's visible workflows in a real Wayland session, repair defects
found at observable boundaries, and retain regression evidence for those
repairs. The changes make linked-worktree actions use and describe the correct
repository, keep notification row actions clickable while scrolling, and retain
config-recovery errors after saved-session restore.

## Goals and non-goals

### Goals

- Verify the native UI, terminal, workspace, pane, worktree, notification,
  agent-hook, browser-feature, persistence, and recovery flows visually.
- Make Create and Attach work when invoked from a linked worktree.
- Make Merge and Remove identify the checkout they actually mutate.
- Keep the notification scrollbar clear of row-level Dismiss buttons.
- Keep the `Config Issue` notification after workspace session restore.

### Non-goals

- Redesign existing screens or rewrite established copy across the app.
- Import real browser data; browser import verification stops at preview.
- Change Chromium keyring behavior. A real profile's cookie preview completed
  slowly while the UI and Cancel action remained responsive.
- Guess the primary checkout for an external `--separate-git-dir` repository
  whose metadata does not expose a verifiable worktree. Mutating operations
  reject that layout instead.

## Scope and approvals

**In scope:** `forktty-core` worktree routing, GTK worktree context and
notification scrolling, regression tests, product contracts, changelog, and
public documentation.

**Out of scope:** packaging behavior, session schema, socket protocol shapes,
browser cookie cryptography, and unrelated visual cleanup.

**Must not change:** worktree identity/transaction invariants, destructive
operation checks, feature boundaries, and GTK 4.14 CSS compatibility.

**Approval required:** N/A; all mutations used disposable repositories and
isolated XDG state. Browser import was not executed.

## Sources and assumptions

- **External behavior source:** the `git2::Repository::commondir`
  documentation defines a linked worktree's common directory as the parent
  repository gitdir; GTK documents overlay scrollbars as drawing over content
  by default.
- **Assumptions:** Linux Wayland desktop with GTK/libadwaita, embedded Ghostty,
  and browser feature dependencies available.
- **Open questions:** none.

## Requirements

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| REQ-001 | Creating from a linked worktree uses the common repository layout and the linked checkout's current HEAD. | Core regression test and manual Create flow. |
| REQ-002 | Attaching from a linked worktree uses the common repository layout. | Core regression test and manual Attach flow. |
| REQ-003 | Merge and Remove show the resolved primary checkout before acting. | GTK regression test and visual dialog checks. |
| REQ-004 | Notification scrolling never overlays the Dismiss controls. | GTK regression test plus populated-panel click check. |
| REQ-005 | The release UI and optional browser UI remain functional across the documented surface matrix, including all four supported agent harnesses. | Manual matrix and repository gates below. |
| REQ-006 | A config quarantine warning remains visible after saved-session restore. | GTK startup regression test, socket inspection, and visual panel check. |

## Acceptance scenarios

### SCN-001: nested worktree operation

- **Given:** an active linked worktree whose HEAD differs from the primary
  checkout.
- **When:** a user creates or attaches another worktree.
- **Then:** Git registers it in the common repository layout; Create starts at
  the active linked checkout's HEAD.
- **Covers:** REQ-001, REQ-002

### SCN-002: destructive dialog context

- **Given:** a linked-worktree workspace is active.
- **When:** a user opens Merge or Remove.
- **Then:** the dialog names the resolved primary checkout used by the action.
- **Covers:** REQ-003

### SCN-003: long notification list

- **Given:** enough notifications to require vertical scrolling.
- **When:** a user scrolls and dismisses a row.
- **Then:** the scrollbar occupies its own gutter and Dismiss remains clickable.
- **Covers:** REQ-004

### SCN-004: application matrix

- **Given:** isolated XDG state and disposable Git/browser fixtures.
- **When:** the release and browser builds are exercised visually.
- **Then:** onboarding, Settings, sidebar modes and placement, command palette,
  panes/tabs, terminal search/menu/copy, notifications, Agent HUD, workspaces,
  worktrees, shortcuts/About, browser navigation/automation/profiles/history/
  bookmarks/import preview, session restore, corrupt-config recovery, and the
  Claude/Codex/Antigravity/OpenCode hook harnesses remain usable.
- **Covers:** REQ-005

### SCN-005: config recovery after session restore

- **Given:** a saved workspace session and a structurally invalid config.
- **When:** ForkTTY quarantines the config and restores the session.
- **Then:** the restored model retains a visible `Config Issue` notification
  naming the quarantined file.
- **Covers:** REQ-006

## Failure, recovery, and edge cases

| ID | Trigger | Required behavior | Recovery or rollback | Covers |
| --- | --- | --- | --- | --- |
| EDGE-001 | Operation starts from a linked checkout. | Resolve Git administration through the common repository without losing the caller's HEAD. | Existing worktree transaction and rollback behavior remains unchanged. | REQ-001, REQ-002 |
| EDGE-002 | Modeled workspace is linked or cannot be resolved. | Show the primary checkout when resolvable; otherwise preserve the stable modeled workspace path. | Existing chooser/error flow remains available. | REQ-003 |
| EDGE-003 | Many notification rows. | Reserve scrollbar width rather than cover row actions. | Dismiss and Clear still refresh immediately. | REQ-004 |
| EDGE-004 | Structurally invalid config. | Quarantine the file, restore or create a usable workspace, and retain the `Config Issue` notification. | The `.bad-*` file preserves the invalid input. | REQ-005, REQ-006 |
| EDGE-005 | External separate git directory has no verifiable primary checkout. | Reject mutating worktree operations instead of guessing from the git-directory parent. | Use a standard checkout or configure verifiable `core.worktree` metadata. | REQ-001, REQ-002, REQ-003 |

Worktree commit points and rollback rules are unchanged; this change only
selects the established common-repository owner before those paths run.

## Security and privacy

- **Trust boundary impact:** none.
- **Input and size limits:** unchanged.
- **Data exposure/storage:** QA used isolated XDG directories and disposable
  repositories; real browser data was discovered and previewed read-only, never
  imported.
- **Command execution:** unchanged; worktree hooks remain validated explicit
  paths and no shell trampoline is introduced.

## Architecture and release impact

- **Owning crate/module:** `forktty-core::worktree`, GTK worktree dialog,
  notification panel, and app bootstrap.
- **Dependency direction:** unchanged.
- **Feature combinations:** `gtk-ghostty` and `browser` both verified.
- **Session/config/socket compatibility:** no format or protocol change.
- **Packaging/runtime:** no loader or artifact-layout change.
- **Public site/docs:** worktree/config behavior synchronized in the docs page,
  both LLM context files, and their contract test in the separate
  `forktty-site` checkout.

## Implementation outline

1. Route linked-worktree Create/Attach administration through the common
   repository while preserving the active source HEAD.
2. Resolve GTK Merge/Remove context to the base checkout and reserve a
   notification scrollbar gutter.
3. Publish config recovery only after session restore has replaced the model.
4. Update contracts and verify the full application matrix and build gates.

## Requirement traceability

| Requirement | Planned test/command | Contract/docs impact | Final evidence |
| --- | --- | --- | --- |
| REQ-001 | `cargo test -p forktty-core create_from_linked_worktree_uses_common_layout_and_current_head` | SPEC, README, CHANGELOG, site | Test passes; manual Create produced a sibling under the primary `.worktrees`. |
| REQ-002 | `cargo test -p forktty-core attach_from_linked_worktree_uses_common_repository_layout` | SPEC, README, CHANGELOG, site | Test passes; manual Attach opened the expected workspace from the primary layout. |
| REQ-003 | `cargo test -p forktty-ui-gtk --no-default-features --features gtk-ghostty remove_and_merge_resolve_base_checkout_from_linked_workspace` | SPEC, CHANGELOG | Test passes; Merge and Remove dialogs visually showed the primary checkout before successful operations. |
| REQ-004 | `cargo test -p forktty-ui-gtk --no-default-features --features gtk-ghostty notification_panel_scrollbar_does_not_overlay_dismiss_buttons` | CHANGELOG | Test passes; populated panel showed a separate scrollbar gutter and Dismiss reduced the count. |
| REQ-005 | full local gate plus manual matrix and `forktty hooks test <provider>` for each provider | This brief | Manual matrix completed; `hooks test claude`, `hooks test codex`, `hooks test antigravity`, and `hooks test opencode` each passed against the isolated live app. The workspace tests/clippy passed for `gtk-ghostty`; 1,026 browser-feature tests passed with 2 profiling tests ignored; browser clippy, both builds, xtask, GTK smoke, desktop validation, deb build/RUNPATH, and `cargo audit` passed. |
| REQ-006 | `cargo test -p forktty-ui-gtk --no-default-features --features gtk-ghostty config_recovery_notification_survives_session_restore` | CHANGELOG, SPEC, site | Test passes; isolated relaunch quarantined `config.toml` and retained `Config Issue` in socket/UI after restoring the saved workspace. |

## Pre-implementation consistency review

- [x] Every requirement is unambiguous and objectively verifiable.
- [x] Primary, alternate, failure, recovery, and relevant non-functional
      scenarios are covered or explicitly excluded.
- [x] Every requirement maps to a planned task and acceptance seam.
- [x] The design follows ForkTTY's crate boundaries, critical constraints, and
      non-goals.
- [x] External behavior claims cite current primary sources.

**Findings:** none.

## Post-implementation convergence review

| Finding | Classification | Requirement/source | Evidence | Resolution |
| --- | --- | --- | --- | --- |
| POST-001 | partial | REQ-005 | Chromium cookie preview took about 50 seconds against a real locked/keyring-backed profile, but completed and Cancel stayed responsive. | Accepted as dataset/keyring-dependent behavior; no import was run and non-cookie preview completed in about one second. |
| POST-002 | missing | REQ-005, EDGE-004 | Runtime recovery quarantined the file but session restore replaced the startup notification. | Fixed by publishing `Config Issue` after restore; regression test and repeat visual/socket check pass. |
| POST-003 | partial | REQ-005 | The first evidence row did not name the four requested harness executions. | Fixed by recording each provider harness in REQ-005 evidence. |
| POST-004 | partial | REQ-001, REQ-002, REQ-003 | An external separate git directory could make the common-directory parent look like a checkout. | Fixed by verifying the candidate checkout maps to the same common repository and rejecting unresolved layouts. |

**Fixed review point:** merge-base with `origin/main` at `da00ff1f`.

**Final verdict:** Ready
