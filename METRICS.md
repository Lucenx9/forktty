# ForkTTY Success Metrics

ForkTTY is alpha, so these metrics are lightweight acceptance checks, not a
telemetry plan. Measure them manually during local dogfooding and release QA
before adding instrumentation. The matching release checklist lives in
[`docs/release-qa.md`](docs/release-qa.md#attention-first-ui-smoke).

## Attention-first UI

Goal: after opening ForkTTY, a user should quickly know which agents or runs
need human attention.

| Metric | Target | How to measure |
| ------ | ------ | -------------- |
| Time to Attention Awareness | <= 3 seconds | Start from a mixed state with agents in `ready`, `working`, `needs_input`, and approval-pending states. Time how long it takes to name every item that needs intervention. |
| Missed Critical States Rate | 0% | In a test state with 5-6 agents/runs, count critical states not noticed on the first scan: `needs_input`, stuck/stale workers, pending approvals, errors, conflicts. |
| Critical Information Scannability | >= 90% visible without scroll | Count attention items visible in the Router rail plus bottom `ATTENTION` feed before scrolling or opening details. |
| Perceived Cognitive Effort | 1-2 on a 5-point scale | After the scan, ask: "How much effort did it take to understand what needs attention?" Keep the answer and screenshot with the QA note. |

## Manual Test Shape

Use this state before judging a UI iteration:

- At least one active worker.
- At least one worker needing input or blocked on approval.
- At least one warning/error/stale/conflict event in the feed.
- At least one routine log/event that should not dominate the attention view.
- Router rail and bottom feed both visible at a normal laptop-width window.

Pass condition: the critical items are visible or obvious within the targets
above, and routine logs do not hide the next human action.

## General UI Visual Quality

Goal: ForkTTY should feel like a quiet Linux-native workbench, not a generic
AI-generated dashboard. Use [docs/DESIGN.md](docs/DESIGN.md) as the visual rule
source.

| Metric | Target | How to measure |
| ------ | ------ | -------------- |
| Visual Clarity Score | 4-5 on a 5-point scale | Ask: "How easy is it to scan the current workspace state?" Test with terminal, sidebar, Router rail, and workflow feed visible. |
| Visual Noise | Low | Count elements that do not help the active workflow: decorative effects, repeated badges, empty sections, redundant labels, or competing accent colors. Target: none in the primary viewport. |
| Consistency and Polish | 4-5 on a 5-point scale | Check spacing rhythm, icon style, hover/focus states, section headers, button sizing, and label tone across the same screen. |
| Critical Elements Visible | >= 90% without scroll | For any screen that reports agent/workflow status, count the critical action rows visible before scrolling or opening a secondary dialog. |
| Contrast and Readability | No obvious failures | Inspect text on dark surfaces, muted labels, selected rows, badges, warning/error states, and disabled controls. Avoid low-contrast gray-on-gray rows. |

## General UI Review Shape

Use this before accepting visible UI changes:

- Open a normal laptop-width window and a wide desktop window.
- Check the default workspace, Settings, Router rail, workflow feed, command
  palette, notifications panel, and at least one dialog touched by the change.
- Confirm the screen follows `docs/DESIGN.md`: one accent, no gradients/glow,
  compact operational layout, sentence-case labels, no emoji, and no unrelated
  widget restyling.
- Save a screenshot when any score is below target, then fix the screen instead
  of accepting the regression as "alpha polish".
