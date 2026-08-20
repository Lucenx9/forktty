# Feature quality brief: deferred workspace spawn rollback

**Status:** Implemented

**Owner:** ForkTTY maintainers

**Source:** User-requested stability and consolidation pass, 2026-08-20

**Related plan:** N/A

## Summary

Keep a newly created workspace provisional until its first terminal has really
materialized. If an accepted GTK terminal spawn fails later, ForkTTY removes
the failed workspace, restores the previously active workspace, persists the
restored state after releasing the completed shared surface-set transaction.

## Goals and non-goals

### Goals

- Give socket and direct GTK workspace creation the same deferred-spawn
  rollback guarantee already used by new tabs and splits.
- Keep session-persistence regression tests isolated from concurrent tests and
  from the developer's real session state.

### Non-goals

- Redesign terminal backend completion semantics.
- Change initial application bootstrap or session-restore recovery.
- Change last-workspace replacement, worktree Create/Attach, or destructive
  close transactions; those require their own commit-after-materialization
  design because their rollback owns additional runtime or filesystem state.

## Scope and approvals

**In scope:** `forktty-socket` workspace creation and deferred compensation,
GTK Open/Create Workspace, focused socket/GTK regression tests, stability
documentation, and release notes.

**Out of scope:** worktree lifecycle, replacement-close transactions, socket
protocol shapes, session format, packaging, and browser behavior.

**Must not change:** worktree/surface guard ordering, terminal failure
recording, GTK 4.14 compatibility, and synchronous spawn error responses.

**Approval required:** N/A; the user requested implementation and a pull
request.

## Sources and assumptions

- **External behavior source:** purely internal behavior; the fix is grounded
  in the local terminal backend interface, GTK command queue, model, and tests.
- **Assumptions:** the GTK backend may accept a spawn before embedded Ghostty
  materializes it, while synchronous adapters disarm deferred compensation
  before returning success.
- **Open questions:** none.

## Requirements

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| REQ-001 | `workspace.create` and `workspace.create_ssh` retain the surface-set guard until terminal materialization; a deferred failure removes the new workspace, restores the previous active workspace, persists the rollback, and releases the guard. | Socket regression test through `dispatch`. |
| REQ-002 | GTK Open Workspace and Create Workspace use the same deferred compensation and do not persist the provisional workspace before materialization. | GTK controller regression test with a queued spawn whose cwd disappears before handling. |
| REQ-003 | Tests that invoke production session persistence use isolated state directories, and test-only environment locking does not replace `PATH` globally. | High-parallelism repeated socket test run. |

## Acceptance scenarios

### SCN-001: socket workspace spawn fails after acceptance

- **Given:** an active workspace and a backend that accepts a new workspace
  spawn but defers materialization.
- **When:** `workspace.create` returns and the backend later reports failure.
- **Then:** only the original workspace remains active, the restored session is
  persisted after the rollback releases its guard, and another surface-set
  transaction can start.
- **Covers:** REQ-001

### SCN-002: GTK open-workspace spawn fails after enqueue

- **Given:** an active workspace and a valid selected directory.
- **When:** GTK queues the new terminal, the directory disappears, and Ghostty
  materialization fails.
- **Then:** the selected workspace is removed, the previous workspace is active,
  and the restored session is persisted without recording the provisional
  workspace.
- **Covers:** REQ-002

### SCN-003: socket tests run concurrently

- **Given:** session-persistence and command-spawning tests in one test process.
- **When:** the socket suite runs at high parallelism.
- **Then:** no test writes another test's session file or temporarily hides
  commands such as `git` and `mkfifo` through a replaced `PATH`.
- **Covers:** REQ-003

## Failure, recovery, and edge cases

| ID | Trigger | Required behavior | Recovery or rollback | Covers |
| --- | --- | --- | --- | --- |
| EDGE-001 | Backend rejects the spawn synchronously. | Preserve the existing structured spawn error. | Deferred compensation runs before the call returns. | REQ-001, REQ-002 |
| EDGE-002 | GTK accepts the command but Ghostty materialization fails. | Do not leave a modeled or persisted dead workspace. | Remove the workspace, restore prior selection, release the guard, then persist. | REQ-001, REQ-002 |
| EDGE-003 | Model lock is poisoned during compensation. | Recover the lock and attempt the complete rollback. | Clear poison only after removing the workspace; log an explicit rollback failure otherwise. | REQ-001, REQ-002 |

The commit point is successful terminal materialization. Before that point the
failure handler owns the surface-set guard and the newly modeled workspace is
provisional.

## Security and privacy

- **Trust boundary impact:** none.
- **Input and size limits:** unchanged.
- **Data exposure/storage:** the test fix prevents writes to the developer's
  real ForkTTY session path.
- **Command execution:** unchanged; commands remain argv-based.

## Architecture and release impact

- **Owning crate/module:** `forktty-socket::workspace_creation` owns deferred
  workspace compensation; socket and GTK workspace creation call its interface.
- **Dependency direction:** unchanged; GTK already depends on `forktty-socket`.
- **Feature combinations:** both `gtk-ghostty` and `browser` must remain green.
- **Session/config/socket compatibility:** no schema or protocol-shape change.
- **Packaging/runtime:** no artifact or Ghostty ABI change.
- **Public site/docs:** the separate site checkout contains unrelated local
  changes, so this task must report the pending `app/docs/page.tsx`,
  `public/llms.txt`, and `public/llms-full.txt` recovery-note update rather than
  mixing worktrees. This brief, `SPEC.md`, and `CHANGELOG.md` record the source
  contract.

## Implementation outline

1. **Socket tracer bullet:** add a failing dispatch-level late-spawn regression,
   then add deferred workspace compensation and route socket create methods.
2. **GTK integration:** route Open/Create Workspace through the same
   compensation and prove the queued-materialization failure path.
3. **Test isolation:** isolate every persistence-writing rollback test and
   remove the test-only global `PATH` replacement.
4. **Convergence:** update contracts and run targeted, feature, stress, and
   review gates.

## Requirement traceability

| Requirement | Planned test/command | Contract/docs impact | Final evidence |
| --- | --- | --- | --- |
| REQ-001 | `cargo test -p forktty-socket workspace_create_deferred_spawn_failure` | `SPEC.md`, `CHANGELOG.md` | Both local and SSH late-failure regressions pass, including restored focus/session, guard release, and poisoned-lock recovery. |
| REQ-002 | `cargo test -p forktty-ui-gtk --no-default-features --features gtk-ghostty open_workspace_` | `SPEC.md`, `CHANGELOG.md` | Both synchronous persistence and deferred GTK rollback pass; the provisional workspace is absent from the pre-materialization session. |
| REQ-003 | repeated `cargo test -p forktty-socket --lib -- --test-threads=64` | N/A | 50/50 high-parallelism full socket-suite iterations pass. |

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
| POST-001 | Resolved | REQ-001–REQ-003 | Full `gtk-ghostty` and browser suites; both clippy gates; 50/50 socket stress; GTK Ghostty smoke; independent standards and spec reviews. | Requirements and recovery paths converge with the implementation and observable tests. |

**Fixed review point:** `7b0afc0d17b0a95d92d38eb621b9cd10329f3a35`

**Final verdict:** Ready
