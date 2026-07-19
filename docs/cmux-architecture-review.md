# cmux architecture review

Research date: 2026-07-15
Official repository: <https://github.com/manaflow-ai/cmux>
Clone inspected: `/tmp/cmux-architecture-review`
Commit inspected: [`3822f1dd475f0c5ddcf961df9f17308c3066ffa1`](https://github.com/manaflow-ai/cmux/commit/3822f1dd475f0c5ddcf961df9f17308c3066ffa1) (`main`, 2026-07-14)
Latest published release at review time: [`v0.64.19`](https://github.com/manaflow-ai/cmux/releases/tag/v0.64.19), nine commits behind the inspected `main`
ForkTTY comparison point: [`2fd3d4cb0e03a8e7ad95d52fd13f3051ec075098`](https://github.com/Lucenx9/forktty/commit/2fd3d4cb0e03a8e7ad95d52fd13f3051ec075098)

## Conclusion

The claim that cmux treats coding agents only as generic terminal processes is **not true for the current codebase**. Its base model is still terminal-first, but cmux now has a substantial agent-specific application layer: typed providers, hook-managed lifecycle, session restore, Feed approvals and telemetry, provider-aware process detection, agent hibernation, native Claude/Codex team panes, agent session panels, and agent skills.

The narrower distinction is real: no provider-neutral task router, durable team/worker/task/message store, or workflow/loop/evidence domain comparable to ForkTTY was found. cmux generally lets Claude, Codex, or OpenCode own the agent run and projects their sessions or subagents into cmux surfaces. ForkTTY additionally owns the coordination model itself.

Therefore, reducing ForkTTY to “match cmux” does **not** imply deleting every agent integration. It supports a more targeted simplification: keep terminal primitives and thin provider adapters, then separately decide whether ForkTTY's product-owned Router, Team, Workflow, and loop state justify their maintenance cost.

## What is factually shared

| Area | cmux at the inspected commit | ForkTTY at the comparison commit |
| --- | --- | --- |
| Terminal shell | Native macOS Swift/AppKit application using libghostty; workspaces, tabs, splits, sidebar, notifications, browser, and socket automation are first-class primitives ([README](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/README.md#L28-L93)). | Native Linux GTK4/libadwaita application with embedded Ghostty, workspaces, panes, splits, sidebar, notifications, browser feature, command palette, and socket automation ([README](https://github.com/Lucenx9/forktty/blob/2fd3d4cb0e03a8e7ad95d52fd13f3051ec075098/README.md#L43-L52)). |
| Generic automation | A large Unix-socket/CLI surface controls windows, workspaces, panes, surfaces, notifications, browser, VM/remote state, Feed, and more ([capability registry](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/Sources/TerminalController%2BCapabilities.swift#L3-L257)). The newer dispatcher is being split by command domain behind a context seam ([coordinator](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/Packages/macOS/CmuxControlSocket/Sources/CmuxControlSocket/Coordinator/ControlCommandCoordinator.swift#L4-L149)). | JSON-RPC Unix socket, CLI, MCP bridge, and typed public method registry. Its public surface includes generic pane/workspace/metadata/notification methods and explicit agent/orchestration methods ([method registry](https://github.com/Lucenx9/forktty/blob/2fd3d4cb0e03a8e7ad95d52fd13f3051ec075098/crates/forktty-socket/src/methods.rs#L60-L140)). |
| Generic attention seam | Standard OSC notifications and generic `notify`, status, progress, and log commands remain useful without an agent ([notification docs](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/docs/notifications.md#L1-L59)). | Notifications and metadata are independent public socket concepts, although orchestration UI consumes them too ([method registry](https://github.com/Lucenx9/forktty/blob/2fd3d4cb0e03a8e7ad95d52fd13f3051ec075098/crates/forktty-socket/src/methods.rs#L71-L82)). |
| Agent lifecycle | Hooks record provider session ID, workspace/surface, PID, lifecycle, and sanitized launch command. cmux can resume sessions and optionally terminate/resume idle background agents ([hook and hibernation contract](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/docs/agent-hooks.md#L1-L110)). | Hook integrations feed explicit agent health, resume, hibernate, and reclaim APIs ([method registry](https://github.com/Lucenx9/forktty/blob/2fd3d4cb0e03a8e7ad95d52fd13f3051ec075098/crates/forktty-socket/src/methods.rs#L65-L70)). |
| Human approvals/feed | `feed.push` converts provider hook payloads into permission, plan, and question cards, records a JSONL audit stream, and can return decisions to the provider ([Feed architecture](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/docs/feed.md#L1-L61)). | Feed approvals and the workflow feed are tied to ForkTTY's workflow/team projections and orchestration rail ([public methods](https://github.com/Lucenx9/forktty/blob/2fd3d4cb0e03a8e7ad95d52fd13f3051ec075098/crates/forktty-socket/src/methods.rs#L63-L64)). |
| Skills/MCP | Publishes agent skills and can discover skill files, but no cmux-owned MCP server was found in the repository. Its primary programmable boundary is CLI/socket ([README](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/README.md#L352-L363)). | Embeds managed agent skills and exposes the socket through an MCP stdio server as a supported product surface. |

## cmux is explicitly agent-aware

These are facts from source, not product-copy interpretation:

- cmux models Codex, Claude Code, and OpenCode as typed `AgentSessionProviderID` cases with distinct executables, launch arguments, transports, and startup rules ([provider model](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/Sources/AgentSessionProvider.swift#L3-L67)).
- Its session index has a typed `SessionAgent` domain and provider-specific resume metadata rather than treating every session as an anonymous PID ([session model](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/Sources/SessionIndexModels.swift#L37-L124), [provider-specific state](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/Sources/SessionIndexModels.swift#L198-L225)).
- The hook integration matrix currently covers seventeen named agent families, with provider-specific install paths, lifecycle events, Feed semantics, and resume commands ([agent hook matrix](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/docs/agent-hooks.md#L16-L50)).
- `cmux claude-teams` enables Claude's experimental teams mode, injects a tmux-compat shim, and adds a system-prompt instruction that steers named teammates into parallel split panes ([launcher policy](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/CLI/CMUXCLI%2BExecutableResolution.swift#L151-L230), [shim and launch](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/CLI/cmux.swift#L20126-L20276)).
- `cmux codex-teams` connects to Codex app-server, tracks parent/child thread identity, role and depth, opens attachable subagents in native split surfaces, and bridges app-server approvals into Feed ([watcher model](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/CLI/cmux.swift#L20279-L20350), [subagent projection](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/CLI/cmux.swift#L20913-L21030), [approval behavior](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/docs/feed.md#L129-L141)).
- Agent hibernation is not a generic terminal pause: it requires a restorable agent session in idle lifecycle state, targets the agent process group, and resumes it through the provider's native session command ([hibernation contract](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/docs/agent-hooks.md#L60-L108)).

**Inference:** cmux's “primitive, not a solution” positioning remains an architectural intention at the workspace/surface layer, but the current product is a hybrid terminal plus agent workbench. The source no longer supports describing the whole application as provider-blind.

## Where cmux remains narrower than ForkTTY

Repository-wide searches for `task.strategy`, orchestration methods, workflow state/types, and provider-neutral worker/team stores found no equivalent to ForkTTY's following domains:

- `TaskStrategy` scores and selects `Solo`, review, parallel research/experiment, or team-pipeline modes and assigns harness roles ([ForkTTY router model](https://github.com/Lucenx9/forktty/blob/2fd3d4cb0e03a8e7ad95d52fd13f3051ec075098/crates/forktty-core/src/task_strategy.rs#L47-L79)).
- `TeamState` durably owns workers, tasks, messages, heartbeats, assignments, reports, and events independently of a provider ([ForkTTY team store](https://github.com/Lucenx9/forktty/blob/2fd3d4cb0e03a8e7ad95d52fd13f3051ec075098/crates/forktty-core/src/team.rs#L1-L149)).
- `WorkflowState` durably owns a goal, plan, evidence, loop stage/iteration/stop reason, and gates ([ForkTTY workflow store](https://github.com/Lucenx9/forktty/blob/2fd3d4cb0e03a8e7ad95d52fd13f3051ec075098/crates/forktty-core/src/workflow.rs#L1-L142)).

cmux does have workspace status and checklists, but they are lightweight workspace UI state rather than an execution strategy or autonomous workflow engine ([CLI contract](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/docs/cli-contract.md#L444-L476)). Its team integrations adapt provider-owned Claude/Codex hierarchies into pane topology; they do not establish a general cmux-owned worker/message/task protocol.

**Inference:** the cleanest conceptual boundary in cmux is not “processes only.” It is “the provider owns reasoning and coordination; cmux owns surfaces, visibility, lifecycle adapters, and user interaction.” ForkTTY crosses that boundary when it chooses strategies and persists provider-neutral execution state.

## Scope comparison

cmux is also not a small reference product at this commit. Beyond the shared terminal shell it contains extensive browser automation, remote SSH/tmux, cloud VMs, macOS/iOS transport, workspace groups/todos, custom sidebars, Dock, Canvas, Feed, session Vault, agent chat/session renderers, and agent hibernation. Its non-debug socket capability array declares 257 methods, including 90 `browser.*` methods ([registry](https://github.com/manaflow-ai/cmux/blob/3822f1dd475f0c5ddcf961df9f17308c3066ffa1/Sources/TerminalController%2BCapabilities.swift#L3-L257)). This does not prove poor performance or poor UX, but it invalidates “cmux is simple, therefore ForkTTY must remove agent support” as an evidence-based argument.

Conversely, ForkTTY exposes 79 core public/connection-level methods (including `events.subscribe`) plus 17 browser methods at the comparison commit; 35 core methods directly belong to `agent.*`, `team.*`, `workflow.*`, `feed.*`, `task.strategy.*`, or `orchestration.*` ([registry](https://github.com/Lucenx9/forktty/blob/2fd3d4cb0e03a8e7ad95d52fd13f3051ec075098/crates/forktty-socket/src/methods.rs#L39-L140)). That is concrete evidence that orchestration increases ForkTTY's contract surface, but cmux is not evidence that all agent-aware functionality must go.

## Recommendation for ForkTTY

1. Do not use “cmux treats agents like generic processes” as the basis for a deletion plan; current cmux contradicts it.
2. Preserve the generic foundation: terminal surfaces, workspace/pane topology, OSC/CLI notifications, status/progress, focus, read/send, and session/layout restore.
3. Preserve only agent adapters that clearly improve those primitives: reliable hook-to-surface binding, attention state, optional native resume, and possibly provider-owned subagent-to-pane projection.
4. Evaluate Router/task strategy, provider-neutral Team state, Workflow loops/evidence, and the orchestration rail as a separate product bet. These are the substantial architectural difference from cmux and the first candidates for freezing or removing if the intended product is a Linux workspace terminal.
5. Treat MCP and managed skills as distribution/integration choices, not as proof of an orchestration domain. They can expose a small terminal API without carrying Router/Team/Workflow state.
6. Measure hidden refresh and runtime cost before making performance claims. This review establishes contract and maintenance complexity, not a measured latency or CPU regression.

The defensible product decision is therefore narrower than “delete the agent domain”: decide whether ForkTTY wants to own **agent coordination policy**. It can stop owning that policy while remaining strongly agent-friendly, which is also the boundary the current cmux implementation most closely demonstrates.
