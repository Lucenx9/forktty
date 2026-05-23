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
- ForkTTY sidebar shows branch/worktree/unread/metadata only.
- Scope:
  - Listening ports: enumerate per-pane child process listeners (e.g. parse `/proc/net/tcp*`
    against the pane child PID tree) and render as sidebar chips.
  - PR status: resolve branch -> PR via `gh` CLI if present, or skip when absent. Cache + bound.
- No new heavy dependency. Pure Rust + existing sidebar model.

## 2. Socket: `events` stream + `capabilities`

- **Impact**: high · **Cost**: medium · **Status**: new
- cmux: `cmux events` streams reconnectable newline-delimited JSON of workspace/surface/focus
  changes; `cmux capabilities` exposes a discovery surface; `cmux rpc <method>` raw passthrough;
  `cmux top` resource usage.
- ForkTTY socket is request/response only; no push/event channel, no capability discovery.
- Scope:
  - Add a `subscribe`/`events` socket verb that holds the connection open and emits bounded
    NDJSON events from the existing model mutation points.
  - Add `capabilities` returning supported verbs + version for forward-compat clients.
- Unblocks external automation and editor/MCP integrations.

## 3. Built-in browser pane (scriptable)

- **Impact**: high · **Cost**: high · **Status**: backlog
- cmux: browser pane with a scriptable API ported from
  [agent-browser](https://github.com/vercel-labs/agent-browser) (Apache-2.0): accessibility-tree
  snapshot, element refs, click, fill forms, evaluate JS. Plus cookie/history import from 20+
  browsers and browser profiles.
- ForkTTY: backlog only.
- Scope (Linux):
  - Embed WebKitGTK 6 (`webkit2gtk-6.0`) as a pane kind alongside VTE.
  - Expose a socket verb set mirroring agent-browser (`snapshot`, `click`, `fill`, `eval`, `goto`).
  - Defer cookie/profile import; ship read/navigate/script first.

## 4. SSH remote workspaces

- **Impact**: medium · **Cost**: medium · **Status**: backlog
- cmux: `cmux ssh user@remote` opens a workspace on a remote host; browser panes route through the
  remote network so localhost works; drag-image uploads via scp.
- Scope: spawn the pane shell as `ssh user@remote`, propagate ForkTTY env over the connection,
  surface remote cwd/branch in the sidebar. Browser routing deferred until #3 lands.

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
  ForkTTY's local-first, no-network, no-telemetry posture.
- libghostty GPU renderer (ForkTTY uses VTE intentionally).
- Sparkle auto-update (ForkTTY ships no update-check by design; AppImage/.deb handle distribution).
