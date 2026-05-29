# cmux Gap Features

Features present in [cmux](https://github.com/manaflow-ai/cmux) (macOS, Swift/AppKit, libghostty)
that ForkTTY (Linux, Rust, GTK4/VTE) does not yet have, ordered by user impact vs. cost.
Source: README, `docs/cli-contract.md`, and `CLI/` of the cmux repo as of 2026-05-23.

This file tracks intent and scope. Implementation status lives in `ROADMAP.md`.

## Legend

- **Impact**: perceived user value.
- **Cost**: implementation effort on the GTK/VTE + Rust stack.
- **Status**: `backlog` (in ROADMAP), `new` (not yet tracked), `partial`.

---

## 1. Sidebar: PR status + listening ports

- **Impact**: high · **Cost**: low · **Status**: **done** (listening ports + PR status)
- cmux sidebar rows show git branch, **linked PR status/number**, working directory,
  **listening ports**, and latest notification text.
- ForkTTY sidebar rows show git branch/worktree, linked PR status/number when enabled, working
  directory, listening ports, unread state, and latest metadata/notification text.
- Scope:
  - Listening ports: enumerate per-pane child process listeners (e.g. parse `/proc/net/tcp*`
    against the pane child PID tree) and render as sidebar chips.
  - PR status: when enabled, resolve branch -> PR via `gh` CLI if present, or skip when absent.
    Cache + bound.
- No new heavy dependency. Pure Rust + existing sidebar model.

## 2. Socket: `events` stream + `capabilities`

- **Impact**: high · **Cost**: medium · **Status**: **done** (events stream + capabilities)
- cmux: `cmux events` streams reconnectable newline-delimited JSON of workspace/surface/focus
  changes; `cmux capabilities` exposes a discovery surface; `cmux rpc <method>` raw passthrough;
  `cmux top` resource usage.
- ForkTTY now ships `events.subscribe` (long-lived NDJSON stream of workspace/surface/focus/
  status/progress/notification/ports/PR changes, computed by a source-agnostic snapshot diff)
  and `system.capabilities` (version + advertised methods). CLI: `forktty events`,
  `forktty capabilities`. `rpc` passthrough and `top` remain out of scope.
- Unblocks external automation and editor/MCP integrations.

## 3. Built-in browser pane (scriptable)

- **Impact**: high · **Cost**: high · **Status**: **SP1+SP2 done**; **SP3 P1/P2 done** (persistent WebKit sessions + profiles); **SP3 P3 core/socket/CLI done** (history/bookmark stores + socket verbs + CLI mirrors); **SP3 P4 done** (browser import); P3 GTK address-bar completion/visit-recording still pending
- cmux: browser pane with a scriptable API ported from
  [agent-browser](https://github.com/vercel-labs/agent-browser) (Apache-2.0): accessibility-tree
  snapshot, element refs, click, fill forms, evaluate JS. Plus cookie/history import from 20+
  browsers and browser profiles.
- ForkTTY SP1 ships a browser pane as a new surface kind (`SurfaceKind::Browser`) embedding WebKitGTK6 behind the `browser` cargo feature, which full source and packaged builds enable by default, with socket verbs `browser.open`/`browser.navigate`, an in-pane address bar (back/forward/reload), and `forktty browser open|navigate` CLI. SP2 ships the scriptable socket verbs (`browser.snapshot`/`browser.click`/`browser.fill`/`browser.eval`) plus socket-driven `back`/`forward`/`reload` via a socket→GTK command channel, with `forktty browser snapshot|click|fill|eval|back|forward|reload` CLI (element refs come from a pragmatic ARIA DOM walk; full AT-SPI is not claimed). SP3 P1/P2 adds persistent per-profile WebKit sessions plus `browser.profile.*` socket/CLI verbs; P3 adds core history/bookmark stores, socket verbs, and CLI mirrors, with GTK completion/visit-recording still pending. P4 adds browser import via the `forktty-import` crate, `browser.import.discover`/`preview`/`run` socket/CLI verbs, and a Settings "Import Browser Data" dialog.
- Scope (Linux):
  - Embed WebKitGTK 6 (`webkitgtk-6.0`) as a pane kind alongside VTE.
  - Expose a socket verb set mirroring agent-browser (`open`, `navigate`, `snapshot`, `click`, `fill`, `eval`).
  - Defer external browser import; ship read/navigate/script/profile first.

## 4. SSH remote workspaces

- **Impact**: medium · **Cost**: medium · **Status**: **done** (core); remote browser routing + scp drag-upload deferred
- cmux: `cmux ssh user@remote` opens a workspace on a remote host; browser panes route through the
  remote network so localhost works; drag-image uploads via scp.
- ForkTTY ships `SurfaceKind::Ssh` panes spawned as `ssh <host>`, the `workspace.create_ssh` socket
  method, a `forktty ssh` CLI, sidebar `ssh:<host>` hints, and respawn-on-restore for remote
  workspaces. Remote browser routing and scp drag-image uploads remain deferred until #3's
  remaining wiring lands.

## 5. Custom project commands (`forktty.json`)

- **Impact**: medium · **Cost**: low · **Status**: new
- cmux: `cmux.json` defines project-specific actions that appear in the command palette.
- Scope: read a repo-local `forktty.json` (bounded, validated argv, no `sh -c`) and inject entries
  into the existing command palette. Reuse the notification_command argv-execution model.

## 6. Deeper agent integration

- **Impact**: medium · **Cost**: medium · **Status**: new
- cmux: `claude-teams` launches Claude Code teammate mode as native splits; `omx`/`omc`/`omo`
  wrappers integrate Codex/Claude/OpenCode with panes.
- ForkTTY ships hook templates but no team/launcher command.
- Scope: a `forktty teams` (or per-agent launcher) that spawns split surfaces wired to the existing
  status/notification metadata pipeline.

## 7. Ghostty config compatibility

- **Impact**: low-medium · **Cost**: low · **Status**: backlog (theme import only)
- cmux reads `~/.config/ghostty/config` for themes/fonts/colors.
- Scope: import Ghostty theme/font/color keys into the VTE palette on startup; map the subset VTE
  supports, ignore the rest.

---

## Out of scope / non-goals

- macOS/Windows builds (ForkTTY is Linux-only by design).
- Cloud VM backend + account/auth + vault (cmux `vm`/`cloud`/`auth`/`vault`): conflicts with
  ForkTTY's local-first posture and no telemetry/update-checks by default.
- Optional user-enabled tools, such as PR resolution through `gh`, may contact external services.
- libghostty GPU renderer (ForkTTY uses VTE intentionally).
- Sparkle auto-update (ForkTTY ships no update-check by design; AppImage/.deb handle distribution).
