# ForkTTY Socket API Stability

ForkTTY is still alpha, but agents and scripts need to know which socket calls
are safe to build on. This page is the stability map for the local
newline-delimited JSON-RPC socket documented in [SPEC.md](../SPEC.md#socket-api).

The source of truth for the advertised method set is
`crates/forktty-socket/src/methods.rs`; this document classifies that public
surface.

## Stability Tiers

| Tier | Contract |
| ---- | -------- |
| Stable-for-alpha | Method name, required parameters, response field meaning, and error code intent should not break within an alpha line without a changelog entry and migration note. New optional fields may be added. Treat human-readable error text as diagnostic, not stable API. |
| Alpha | Public and supported, but the shape may still change between alpha releases as the feature hardens. Changes must be documented. |
| Source-only / experimental | Available only in explicit source builds or behind feature flags. Do not rely on it for release artifacts. |
| Internal | Not part of the public automation contract even if it exists in code. |

## Stable-for-alpha Core

Use these first for scripts, MCP clients, hooks, and agent coordination:

| Area | Methods |
| ---- | ------- |
| Discovery | `system.ping`, `system.identify`, `system.capabilities`, `system.top` |
| Context | `context.snapshot` |
| Workspace | `workspace.list`, `workspace.create`, `workspace.create_ssh`, `workspace.select`, `workspace.close` |
| Surface | `surface.list`, `surface.read_text`, `surface.capture_tail`, `surface.send_text`, `surface.split`, `surface.focus`, `surface.close` |
| Pane tabs | `pane.new_tab`, `pane.select_tab` |
| Worktree reads | `worktree.list`, `worktree.status` |
| Notifications | `notification.create`, `notification.list`, `notification.clear` |
| Agent state | `agent.list`, `agent.health`, `agent.resume` |
| Status and logs | `status.summary`, `metadata.set_status`, `metadata.list_status`, `metadata.clear_status`, `metadata.set_progress`, `metadata.list_progress`, `metadata.clear_progress`, `metadata.log`, `metadata.list_logs`, `metadata.clear_logs` |
| Project actions | `project.action.list`, `project.action.run` |
| Topology and remotes | `topology.tree`, `remote.list`, `remote.status` |
| Feed reads and responses | `feed.list`, `feed.approval.respond` |

## Alpha Orchestration and Mutation

These are public, tested, and user-facing, but still evolving with the router,
workflow loop, and team model:

| Area | Methods |
| ---- | ------- |
| Task router | `task.strategy.plan`, `task.strategy.apply` |
| Worktree mutation | `worktree.create`, `worktree.attach`, `worktree.remove`, `worktree.merge` |
| Workflow | `workflow.list`, `workflow.get`, `workflow.upsert`, `workflow.plan.set`, `workflow.loop.set`, `workflow.evidence.add`, `workflow.replay` |
| Team | `team.list`, `team.get`, `team.upsert`, `team.finish`, `team.worker.upsert`, `team.worker.heartbeat`, `team.worker.launch`, `team.worker.health`, `team.worker.nudge`, `team.worker.shutdown`, `team.task.upsert`, `team.message.send`, `team.message.dispatch`, `team.message.ack`, `team.inbox`, `team.summary`, `team.events` |
| Cleanup | `orchestration.cleanup` |
| Events | `events.subscribe` |
| Agent lifecycle maintenance | `agent.hibernate`, `agent.reclaim.plan`, `agent.reclaim` |

## Source-only / Experimental

Browser methods are source-only behind the `browser` feature and are not shipped
in AppImage or Debian release artifacts:

`browser.open`, `browser.navigate`, `browser.snapshot`, `browser.click`,
`browser.fill`, `browser.back`, `browser.forward`, `browser.reload`,
`browser.profile.list`, `browser.profile.create`, `browser.profile.delete`,
`browser.history.list`, `browser.history.search`, `browser.history.clear`,
`browser.bookmark.add`, `browser.bookmark.list`, `browser.bookmark.remove`.

## Change Rules

- Additive response fields are allowed in any tier.
- Removing a method, renaming a method, changing required parameters, or changing
  the meaning of a stable-for-alpha response field requires a `CHANGELOG.md`
  entry and a SPEC update.
- MCP tool schemas, CLI wrappers, `SPEC.md`, this file, and
  `forktty-site` agent context should move together when a public method's
  behavior changes.
- Prefer `system.identify` before mutating calls so stale workspace or surface
  ids are detected early.
- Prefer `context.snapshot` for agent monitoring and `surface.read_text` /
  `surface.capture_tail` only when terminal text is actually needed.
