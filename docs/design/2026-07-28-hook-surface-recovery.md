# Feature quality brief: hook surface target recovery

**Status:** Implemented

**Owner:** ForkTTY maintainers

**Source:** User-provided hook audit and accepted implementation review, 2026-07-28

**Related plan:** N/A

## Summary

Hook lifecycle status updates recover their owning terminal surface when the
hook retains a valid workspace or session cwd but omits or carries a stale
surface id. Successful recovery keeps the surface's persisted agent session
and the workspace sidebar status aligned.

## Goals and non-goals

### Goals

- Recover a unique live hook surface inside an explicitly selected workspace.
- Treat a stale supplied surface id as recoverable when the learned target or
  session cwd identifies one unique live replacement.
- Keep explicit workspace selection authoritative during fallback.
- Reject primary agent hook statuses that cannot bind to a live surface instead
  of persisting metadata that the sidebar must hide.

### Non-goals

- Supporting multiple simultaneous agent sessions on one surface.
- Changing suspended-session tombstone behavior or its sidebar presentation.
- Changing generated provider hook shell commands or failure exit policy.
- Changing generic `metadata.set_status` calls that omit `hook_session_id`.

## Scope and approvals

**In scope:** `forktty-socket` hook target resolution, socket regression tests,
the socket/spec contract, changelog, and matching public hook documentation.

**Out of scope:** GTK rendering code, session format, provider configuration,
worktree behavior, dependencies, packaging, and release automation.

**Must not change:** owner-only socket access, metadata validation and size
limits, hook ordering/watermarks, suspended-session suppression, prompt
correlation cleanup, and Codex process-provenance safeguards.

**Approval required:** N/A; the user approved this implementation scope.

## Sources and assumptions

- **External behavior source:** Purely internal ForkTTY behavior, grounded in
  the accepted audit, local socket code, and executable regression probes.
- **Assumptions:** Hook cwd is an existing absolute directory; recovery remains
  best-effort and requires exactly one eligible live surface.
- **Open questions:** None. Multi-agent ownership and suspended presentation are
  intentionally separate follow-ups.

## Requirements

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| REQ-001 | A primary agent hook status with explicit workspace, no surface id, and a uniquely matching cwd binds that workspace's surface and persists the status. | Socket regression test through `metadata.set_status`, followed by `agent.list` and `metadata.list_status`. |
| REQ-002 | A stale supplied surface id recovers through a live learned target or a unique cwd match and does not fail the status mutation. | Socket regression tests for learned-target and cwd recovery. |
| REQ-003 | Explicit workspace selection constrains all fallback candidates; an equally named cwd in another workspace cannot be selected or overwrite the explicit workspace. | Socket cross-workspace and ambiguity regression tests. |
| REQ-004 | An unresolved primary `agent:<provider>` hook status is rejected without persisting status metadata or advancing the hook target/watermark. | Socket failure-path regression test followed by corrected retry using the same event order. |
| REQ-005 | Existing unscoped hook-session fallback, Codex provenance checks, generic status calls, and suspended-session behavior remain unchanged. | Existing targeted socket tests and workspace test suite. |

## Acceptance scenarios

### SCN-001: Explicit workspace and unique cwd

- **Given:** A live surface in workspace A and a hook status carrying workspace
  A, session id, and that surface's cwd but no surface id.
- **When:** `metadata.set_status` is dispatched.
- **Then:** The status succeeds and the surface owns the reported agent session.
- **Covers:** REQ-001

### SCN-002: Stale surface recovery

- **Given:** A valid workspace, a stale surface id, and either a previously
  learned live target or one unique live same-cwd surface.
- **When:** The next hook status is dispatched.
- **Then:** The stale id is replaced by the recovered target and the status is
  applied once.
- **Covers:** REQ-002

### SCN-003: Explicit workspace isolation

- **Given:** Workspace A is explicit while only workspace B has the supplied
  cwd, or workspace A contains multiple matching live surfaces.
- **When:** The hook publishes a primary agent status.
- **Then:** The request returns `not_found` or `conflict`, does not write either
  workspace's status, and does not bind a surface.
- **Covers:** REQ-003, REQ-004

### SCN-004: Corrected retry

- **Given:** An unresolved primary status was rejected at event order N.
- **When:** The hook retries order N with a unique valid target.
- **Then:** The retry applies, proving the rejected attempt did not advance the
  in-memory ordering watermark.
- **Covers:** REQ-004

## Failure, recovery, and edge cases

| ID | Trigger | Required behavior | Recovery or rollback | Covers |
| --- | --- | --- | --- | --- |
| EDGE-001 | Explicit workspace does not exist. | Preserve the existing workspace `not_found` response. | No mutation or learned-target change. | REQ-003 |
| EDGE-002 | Cwd has no live match in the explicit workspace. | Reject a primary agent status with `not_found`. | A corrected retry at the same order remains applicable. | REQ-003, REQ-004 |
| EDGE-003 | Cwd matches multiple surfaces inside the explicit workspace. | Reject with `conflict`. | No mutation or learned-target change. | REQ-003, REQ-004 |
| EDGE-004 | Learned target belongs to another explicit workspace. | Do not reuse or erase it for this failed request. | Preserve the prior session target for later correctly scoped hooks. | REQ-003 |
| EDGE-005 | Hook mutation is a workspace-scoped log/notification rather than a primary status. | Preserve existing workspace-scoped behavior when no surface can be inferred. | N/A | REQ-005 |

The commit point remains the existing serialized mutation call. Target
resolution and validation complete before the model mutation and before the
watermark/learned target are committed, so failures require no rollback.

## Security and privacy

- **Trust boundary impact:** No new caller authority. Recovery remains inside
  the owner-only Unix socket and never crosses an explicit workspace selector.
- **Input and size limits:** Existing hook id, cwd, metadata, and request bounds
  remain unchanged.
- **Data exposure/storage:** No new data or persistence.
- **Command execution:** None. Codex `/proc` process matching remains unchanged
  except that candidates may be narrowed to the explicit workspace first.

## Architecture and release impact

- **Owning crate/module:** `crates/forktty-socket/src/hook_session.rs` and
  `crates/forktty-socket/src/tests/metadata_hooks.rs`.
- **Dependency direction:** Unchanged; target inference remains in the socket
  layer over `forktty-core` model state.
- **Feature combinations:** Socket behavior is feature-neutral; both
  `gtk-ghostty` and `browser` application builds remain required.
- **Session/config/socket compatibility:** Additive recovery behavior for
  existing fields; no JSON shape, method, session, or config migration.
- **Packaging/runtime:** N/A.
- **Public site/docs:** Update `SPEC.md`, `CHANGELOG.md`, and the site's hook
  explanation/LLM context; no install-flow change.

## Implementation outline

1. **Slice 1:** Add socket regression tests for explicit-workspace/no-surface
   recovery, then implement workspace-constrained cwd lookup.
2. **Slice 2:** Add stale-surface learned/cwd recovery and failure/retry tests,
   then make stale targets fall through to the same safe resolver.
3. **Slice 3:** Update contracts and run feature-neutral plus both application
   feature-combination gates.

## Requirement traceability

| Requirement | Planned test/command | Contract/docs impact | Final evidence |
| --- | --- | --- | --- |
| REQ-001 | `cargo test -p forktty-socket explicit_workspace_hook_status` | `SPEC.md`, changelog, site hooks text | `explicit_workspace_hook_status_learns_unique_surface_from_cwd` and `explicit_workspace_name_hook_status_learns_unique_surface_from_cwd` pass; the ID case verifies both `agent.list` and `metadata.list_status`. |
| REQ-002 | `cargo test -p forktty-socket stale_surface_hook_status` | `SPEC.md`, changelog, site hooks text | Three stale-surface recovery tests pass for explicit cwd, unscoped cwd, and a learned live target. |
| REQ-003 | `cargo test -p forktty-socket explicit_workspace_hook_target` | `SPEC.md` | Cross-workspace learned/cwd and same-workspace ambiguity tests pass; alternate selectors normalize to one canonical workspace ID. |
| REQ-004 | `cargo test -p forktty-socket unresolved_agent_hook_status` | `SPEC.md` | The unresolved request leaves status empty and its corrected same-order retry binds the surface. |
| REQ-005 | Targeted metadata hook tests plus workspace/browser gates | N/A | All 37 metadata hook tests pass; full `gtk-ghostty` and serial browser suites, clippy, and both builds pass. |

## Pre-implementation consistency review

- [x] Every requirement is unambiguous and objectively verifiable.
- [x] Primary, alternate, failure, recovery, and relevant non-functional
      scenarios are covered or explicitly excluded.
- [x] Every requirement maps to a planned task and acceptance seam.
- [x] The design follows ForkTTY's crate boundaries, critical constraints, and
      non-goals.
- [x] External behavior claims cite current primary sources.

**Findings:** None. The accepted audit was narrowed so explicit workspace
selection remains authoritative and unresolved agent statuses cannot recreate
the original hidden-metadata state.

## Post-implementation convergence review

| Finding | Classification | Requirement/source | Evidence | Resolution |
| --- | --- | --- | --- | --- |
| Recovered targets added `workspace_id` without removing alternate explicit selectors. | Implementation defect | REQ-001, REQ-003 | RED regression returned `Ambiguous workspace selector` for `workspace_name`; it passes after canonicalization. | Recovery now removes workspace/surface aliases before inserting the canonical target. |
| A failed recovery could clear a stale learned target before the mutation commit. | Implementation defect | EDGE-003 and commit-point invariant | Review traced the early assignment in `live_learned_hook_session_target`. | Learned-target lookup is immutable; only the existing post-mutation commit updates ingress state. |
| Site wording implied an explicit workspace was always required. | Documentation drift | REQ-002, public-site alignment rule | Code and regression tests also recover an unscoped stale surface by unique cwd. | Site docs now say an explicit workspace constrains recovery when supplied. |
| Resolver fallback and new socket setup/assertions were duplicated. | Quality finding | Repository surgical/modularity guidance | Standards review identified repeated learned/cwd branches and repeated public-boundary fixtures. | Extracted one recovery helper and small test-only workspace/status/assertion helpers. |
| REQ-001 evidence did not inspect persisted status metadata. | Evidence gap | REQ-001 acceptance evidence | Initial test asserted only the response and `agent.list`. | The ID-selector regression now also asserts `metadata.list_status`. |
| Hook target normalization duplicated the parser's selector alias list. | Quality finding | Parameter ownership and LLM-readable-code guidance | Follow-up standards review showed a future alias would require coordinated edits. | Selector aliases and canonical replacement now share one owner in `param_helpers.rs`. |

**Fixed review point:** `fc2f3ddb02c0c0e9d910e57c96186abba4eb8d03`

**Final verdict:** Ready
