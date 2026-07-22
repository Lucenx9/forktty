# Feature quality brief: [feature name]

Use this template for non-trivial changes that cross modules, alter observable
behavior, or affect socket, session, config, security, worktree, packaging, or
release contracts. A focused bug fix does not need a new brief when its issue or
test already provides equivalent scope and acceptance evidence.

Delete instructional comments and sections that are genuinely not applicable.

**Status:** Draft | Approved | Implemented

**Owner:** [name or handle]

**Source:** [issue, discussion, or request]

**Related plan:** [path or N/A]

## Summary

[What changes, for whom, and why? Keep implementation details out of this
section.]

## Goals and non-goals

### Goals

- [Observable outcome]

### Non-goals

- [Adjacent behavior intentionally excluded]

## Scope and approvals

**In scope:** [modules, behavior, and artifacts]

**Out of scope:** [explicit boundaries]

**Must not change:** [files, contracts, or invariants that are negative space]

**Approval required:** [security, destructive operations, release policy, or N/A]

## Sources and assumptions

- **External behavior source:** [official documentation, upstream source/issue,
  standard, or “purely internal behavior”]
- **Assumptions:** [environment, platform, data, compatibility]
- **Open questions:** [items that materially change the design]

## Requirements

Use stable IDs. Do not renumber accepted requirements; add or retire IDs with a
short reason.

| ID | Requirement | Acceptance evidence |
| --- | --- | --- |
| REQ-001 | [Specific observable behavior] | [Test, command, or manual scenario] |

## Acceptance scenarios

### SCN-001: [primary or independently testable scenario]

- **Given:** [initial state]
- **When:** [action]
- **Then:** [observable result]
- **Covers:** REQ-001

## Failure, recovery, and edge cases

| ID | Trigger | Required behavior | Recovery or rollback | Covers |
| --- | --- | --- | --- | --- |
| EDGE-001 | [failure/boundary] | [observable result] | [restored state or explicit terminal error] | REQ-001 |

For state mutations, define the commit point, rollback behavior, and what is
reported if rollback also fails.

## Security and privacy

- **Trust boundary impact:** [none or exact change]
- **Input and size limits:** [unchanged or exact validation]
- **Data exposure/storage:** [none or exact data and retention]
- **Command execution:** [argv-only behavior, approval boundary, or N/A]

## Architecture and release impact

- **Owning crate/module:** [path]
- **Dependency direction:** [why existing crate flow remains intact]
- **Feature combinations:** `gtk-ghostty`, `browser`, or N/A
- **Session/config/socket compatibility:** [impact and migration]
- **Packaging/runtime:** [deb, AppImage, loader, Ghostty ABI, or N/A]
- **Public site/docs:** [exact files or N/A with reason]

## Implementation outline

List dependency-ordered slices. Each slice must be independently reviewable and
must name its expected files and verification seam.

1. **Slice 1:** [behavior and paths]
2. **Slice 2:** [behavior and paths]

## Requirement traceability

Complete the planned columns before implementation and the final-evidence
column before review.

| Requirement | Planned test/command | Contract/docs impact | Final evidence |
| --- | --- | --- | --- |
| REQ-001 | [test or command] | [SPEC/README/CHANGELOG/site/path or N/A] | [passing test, command output, screenshot, or finding] |

## Pre-implementation consistency review

Record findings rather than silently rewriting the brief.

- [ ] Every requirement is unambiguous and objectively verifiable.
- [ ] Primary, alternate, failure, recovery, and relevant non-functional
      scenarios are covered or explicitly excluded.
- [ ] Every requirement maps to a planned task and acceptance seam.
- [ ] The design follows ForkTTY's crate boundaries, critical constraints, and
      non-goals.
- [ ] External behavior claims cite current primary sources.

**Findings:** [none, or `PRE-001` entries with resolution]

## Post-implementation convergence review

Compare the accepted brief, implementation, tests, and documentation. Classify
each gap as `missing`, `partial`, `contradicts`, or `unrequested`.

| Finding | Classification | Requirement/source | Evidence | Resolution |
| --- | --- | --- | --- | --- |
| POST-001 | [classification] | [REQ/source] | [path/test/diff] | [fixed, accepted, or follow-up] |

**Fixed review point:** [commit, merge-base, or original uncommitted baseline]

**Final verdict:** Ready | Not ready
