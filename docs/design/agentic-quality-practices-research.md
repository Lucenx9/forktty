# Agentic and spec-driven quality practices for ForkTTY

**Date:** 2026-07-22

**Scope:** Evaluate `rohitg00/ai-engineering-from-scratch` and
`github/spec-kit` for practices that can improve ForkTTY's engineering quality.

**Method:** Primary sources only: repository documentation, source, workflows,
and history. GitHub references are pinned to commits rather than `main`.

Reviewed snapshots:

- [`ai-engineering-from-scratch` @ `c8b9b92`](https://github.com/rohitg00/ai-engineering-from-scratch/tree/c8b9b9244f3210b840776675175662dacf208264)
- [`spec-kit` @ `3356161`](https://github.com/github/spec-kit/tree/3356161d88c4140d054863c95d7ed217d967c907);
  the stable release available at review time was
  [`v0.13.4`](https://github.com/github/spec-kit/releases/tag/v0.13.4)

## Conclusion

Both repositories contain useful practices, but ForkTTY should adapt a small
subset to its existing workflow rather than install either system wholesale.

The highest-value changes are:

1. turn every release-critical, incident-derived rule into a deterministic
   check with a named owner and gate;
2. trace requirements through acceptance scenarios, tests, and documentation
   for non-trivial changes;
3. run a consistency review before implementation and a spec-to-code coverage
   review before final approval;
4. standardize the design briefs already used by ForkTTY without introducing
   `.specify/`, a task store, or a workflow engine.

## Implementation status

This change applies the first practical slice of those recommendations:

- `xtask check` now enforces the paired vendored libghostty paths,
  `ReleaseSafe` optimization, and the native `-Dcpu=baseline` patch;
- the socket protocol tests require every advertised capability method to be
  classified in `docs/socket-api.md`;
- `docs/feature-quality-template.md` supplies the lightweight brief and the
  pre/post implementation review structure;
- ForkTTY-specific bug-report and pull-request templates capture the evidence
  needed for diagnosis and review.

Broader lifecycle/documentation mapping should remain incremental: add a
declared relationship only when it can model a real one-to-one or
one-to-many contract without false positives.

### Release-invariant ownership

The repository's default code owner, `@Lucenx9`, owns these release-critical
rules through `.github/CODEOWNERS`. Their source and gate are explicit so a
future maintainer can locate both sides of each contract.

| Invariant | Source of truth | Gate |
| --- | --- | --- |
| Paired vendored `libghostty-vt` patch paths | Root `Cargo.toml` | `xtask/src/libghostty_release.rs` via `cargo run -p xtask -- check` |
| `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseSafe` | `.cargo/config.toml` | `xtask/src/libghostty_release.rs` via `cargo run -p xtask -- check` |
| Native Zig builds use `-Dcpu=baseline` | `vendor/libghostty-rs/crates/libghostty-vt-sys/build.rs` | `xtask/src/libghostty_release.rs` via `cargo run -p xtask -- check` |
| Advertised socket methods belong to exactly one stability tier | `crates/forktty-socket/src/methods.rs` and `docs/socket-api.md` | `advertised_socket_methods_are_classified_in_stability_docs` |

## Adopt

### 1. Make release-critical constraints executable

The most relevant lesson from `ai-engineering-from-scratch` separates
operational constraints from prose and associates each rule with a check. Its
example implements checks for state, forbidden files, tests, and approval
boundaries
([documentation](https://github.com/rohitg00/ai-engineering-from-scratch/blob/c8b9b9244f3210b840776675175662dacf208264/phases/14-agent-engineering/33-instructions-as-executable-constraints/docs/en.md#L35-L53),
[source](https://github.com/rohitg00/ai-engineering-from-scratch/blob/c8b9b9244f3210b840776675175662dacf208264/phases/14-agent-engineering/33-instructions-as-executable-constraints/code/main.py#L22-L49)).

ForkTTY already has the right foundation in `cargo run -p xtask -- check` and
several deterministic protections. In particular,
`chrome_micro_polish_css_stays_gtk_414_compatible` rejects CSS custom
properties, while the `xtask` tests exercise `-Dcpu=baseline` in the Ghostty
packaging probe.

The first implementation slice inventories the following critical invariants
under `xtask check`:

- both `[patch.crates-io]` entries, `libghostty-vt` and
  `libghostty-vt-sys`, must point to their expected paths in the same vendored
  tree;
- `.cargo/config.toml` must keep
  `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseSafe`;
- the vendored `libghostty-vt-sys/build.rs` must keep the native-build
  `-Dcpu=baseline` patch;
- existing protections, including the GTK 4.14 CSS test, should remain listed
  in the invariant inventory without duplicating their implementation.

Two follow-up applications of the same principle are worthwhile:

- derive an in-repository drift check from the socket/lifecycle method
  registry. It should validate the declared, potentially non-one-to-one
  relationships between capabilities, dispatch, CLI/help, hooks, `SPEC.md`,
  `README.md`, and `docs/socket-api.md`. The curriculum repository uses a
  source-derived catalog for the same reason
  ([catalog construction](https://github.com/rohitg00/ai-engineering-from-scratch/blob/c8b9b9244f3210b840776675175662dacf208264/scripts/build_catalog.py#L6-L18),
  [entry derivation](https://github.com/rohitg00/ai-engineering-from-scratch/blob/c8b9b9244f3210b840776675175662dacf208264/scripts/build_catalog.py#L247-L280)).
  The public website is a separate checkout and therefore needs a matching
  check in its own repository or an explicit task-level verification;
- continue converting real worktree, rollback/respawn, event, hook, and JSON
  regressions into deterministic end-to-end cases. ForkTTY already has many of
  these tests; the objective is a coverage map that exposes gaps, not a second
  duplicate suite. The eval lesson distinguishes final-state and trajectory
  evidence and recommends preserving failure cases as regression tests
  ([eval layers](https://github.com/rohitg00/ai-engineering-from-scratch/blob/c8b9b9244f3210b840776675175662dacf208264/phases/14-agent-engineering/30-eval-driven-agent-development/docs/en.md#L23-L53)).

### 2. Use stable requirement IDs and a requirement-to-evidence matrix

Spec Kit's feature template requires prioritized, independently testable user
stories, Given/When/Then scenarios, edge cases, `FR-###` requirements, and
measurable `SC-###` outcomes
([scenarios](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/templates/spec-template.md#L11-L37),
[requirements and criteria](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/templates/spec-template.md#L71-L118)).

ForkTTY does not need the full template. A non-trivial design brief only needs
these mandatory sections:

- goal, scope, assumptions, and non-goals;
- requirements with stable IDs;
- acceptance scenarios and edge cases, including rollback and failure paths
  for worktree, socket, session, and packaging changes;
- a `requirement -> test/command -> documentation/SPEC/site` traceability
  table.

This complements the existing design and implementation plans under
`docs/superpowers/`. Their file maps and regression coverage are generally
strong; stable IDs make completeness checks mechanical.

### 3. Separate pre-implementation consistency from post-implementation coverage

Spec Kit distinguishes two useful questions:

- before code, `analyze` is read-only and detects ambiguity, duplication,
  terminology drift, conflicts, and requirements without tasks
  ([contract](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/templates/commands/analyze.md#L52-L60),
  [analysis passes](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/templates/commands/analyze.md#L106-L160));
- after implementation, `converge` classifies gaps as `missing`, `partial`,
  `contradicts`, or `unrequested`, links them to their source, and does not add
  work when nothing remains
  ([classification](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/templates/commands/converge.md#L133-L176),
  [append-only output](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/templates/commands/converge.md#L196-L225)).

ForkTTY should adapt this as two lightweight, read-only reviews:

- a brief consistency review before implementing a complex change;
- a final requirement-to-evidence review that also searches for behavior that
  was not requested.

The review should report gaps. It should not automatically create tasks or
rewrite accepted specifications.

## Adapt

### 4. Declare ForkTTY's hybrid specification lifecycle

Spec Kit describes flow-back, flow-forward, and living specifications. It
explicitly treats the choice as a team convention rather than a CLI setting
([models](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/docs/concepts/spec-persistence.md#L26-L84),
[selection](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/docs/concepts/spec-persistence.md#L86-L106)).

ForkTTY already uses a sensible hybrid model and should make it explicit:

- `SPEC.md` is the living contract for product behavior, socket methods,
  configuration, session format, and security boundaries;
- completed design briefs are historical decision records;
- plans and checklists are derived operational artifacts, not competing
  normative sources.

### 5. Decompose only genuine epics into independently testable slices

The Spec Kit "spec of specs" guide recommends independently testable slices,
explicit boundaries, dependency ordering, separate worktrees for parallel
work, and bidirectional references with stable IDs
([decomposition](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/docs/concepts/spec-of-specs.md#L20-L39),
[traceability](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/docs/concepts/spec-of-specs.md#L45-L73)).

This fits changes that cross core, socket, GTK, packaging, and the public site.
It should be reserved for genuine epics because the guide itself identifies it
as the highest-overhead option
([scope warning](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/docs/concepts/spec-of-specs.md#L10-L18)).

### 6. Capture scope and rollback without adding another store

The curriculum's scope-contract example records a goal, allowed and forbidden
files, acceptance criteria, rollback, and approval boundaries, then compares
them with touched files and executed commands
([schema](https://github.com/rohitg00/ai-engineering-from-scratch/blob/c8b9b9244f3210b840776675175662dacf208264/phases/14-agent-engineering/36-scope-contracts/code/main.py#L22-L42),
[verification](https://github.com/rohitg00/ai-engineering-from-scratch/blob/c8b9b9244f3210b840776675175662dacf208264/phases/14-agent-engineering/36-scope-contracts/code/main.py#L114-L160)).

For ForkTTY, forbidden scope, acceptance commands, and rollback plans are
valuable for destructive or release-sensitive changes. They belong in the
existing Markdown brief or plan, not a new `scope_contract.json` store.

### 7. Add small, ForkTTY-specific GitHub templates

Spec Kit's bug form captures reproduction, expected and actual behavior,
version, environment, logs, and context
([form](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/.github/ISSUE_TEMPLATE/bug_report.yml#L11-L57),
[environment and logs](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/.github/ISSUE_TEMPLATE/bug_report.yml#L104-L135)).

Before this change, ForkTTY had no issue or PR template even though `AGENTS.md`
already defined their desired contents and `CONTRIBUTING.md` told contributors
to fill in a PR template. The new ForkTTY bug form additionally captures:

- distro and version;
- Wayland, X11, or headless session;
- AppImage, Debian package, or source build;
- `gtk-ghostty` or `browser` feature;
- relevant `forktty doctor` output.

Spec Kit's PR template is too small for ForkTTY because it does not cover
release, documentation, or public-site impact
([template](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/.github/PULL_REQUEST_TEMPLATE.md#L1-L21)).
It is useful only as a minimal structural reference.

## Avoid or treat as out of scope

### Do not install Spec Kit wholesale

Spec Kit creates its own hierarchy of constitutions, specs, plans, tasks,
command templates, extensions, and presets
([base workflow](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/README.md#L86-L126),
[override stack](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/README.md#L189-L204)).
In ForkTTY this would duplicate `AGENTS.md`, `SPEC.md`, local skills, and the
existing design/plan artifacts.

The upstream task template also makes tests optional unless explicitly
requested
([source](https://github.com/github/spec-kit/blob/3356161d88c4140d054863c95d7ed217d967c907/templates/tasks-template.md#L8-L20)),
which conflicts with ForkTTY's observable regression-coverage expectations.

The project is evolving rapidly before 1.0: `converge` was introduced on
2026-06-17
([commit](https://github.com/github/spec-kit/commit/0c29d890abbdcda3acefab724e0b1bf05a52ddf8)),
and the "spec of specs" guide landed on the date of this review
([commit](https://github.com/github/spec-kit/commit/bb5a2c54241c97c9e7ac836acbe7b5884f3f538c)).
Adopting the concepts before the tooling therefore reduces churn.

### Do not turn ForkTTY into an agent workbench

The curriculum proposes JSONL command records with output, exit codes, and
timings
([feedback record](https://github.com/rohitg00/ai-engineering-from-scratch/blob/c8b9b9244f3210b840776675175662dacf208264/phases/14-agent-engineering/37-runtime-feedback-loops/code/main.py#L42-L55),
[runner](https://github.com/rohitg00/ai-engineering-from-scratch/blob/c8b9b9244f3210b840776675175662dacf208264/phases/14-agent-engineering/37-runtime-feedback-loops/code/main.py#L103-L161))
and aggregated verification reports with signed overrides
([gate](https://github.com/rohitg00/ai-engineering-from-scratch/blob/c8b9b9244f3210b840776675175662dacf208264/phases/14-agent-engineering/38-verification-gates/code/main.py#L59-L95)).

The principles are useful: do not claim success without an exit status, and
keep deterministic evidence separate from qualitative review. As product
features, however, these artifacts would become a provider-neutral task and
evidence store plus an execution engine, which ForkTTY explicitly excludes.

### Do not treat educational examples as production components

The curriculum requires at least five tests per lesson and documents manual
per-PR validation
([contract](https://github.com/rohitg00/ai-engineering-from-scratch/blob/c8b9b9244f3210b840776675175662dacf208264/AGENTS.md#L102-L126)),
but its checked-in CI runs structural audits and synchronization checks rather
than every lesson demo and test suite
([workflow](https://github.com/rohitg00/ai-engineering-from-scratch/blob/c8b9b9244f3210b840776675175662dacf208264/.github/workflows/curriculum.yml#L32-L45)).
The audit checks structure, minimum presence, quiz schemas, and internal links,
not code semantics
([source](https://github.com/rohitg00/ai-engineering-from-scratch/blob/c8b9b9244f3210b840776675175662dacf208264/scripts/audit_lessons.py#L97-L126)).

The reviewed snapshot's phase 14 audit, syntax runner, and 42 demos passed
locally. That shows the examples are executable, not that they are a verified
library suitable for direct integration. ForkTTY should reimplement selected
ideas in Rust/`xtask` and cover them through its own gates.

## Delivery sequence

1. **Delivered:** extend the existing invariant gate with the missing
   libghostty release constraints.
2. **Delivered:** add a small Markdown feature-quality template for complex
   changes.
3. **Delivered:** add requirement-to-evidence coverage and
   unrequested-behavior checks to the documented final review workflow.
4. **Delivered:** add ForkTTY-specific bug and pull-request templates.
5. **Deferred deliberately:** after several real changes, reassess whether
   broader lifecycle/documentation automation pays for its maintenance cost.
