# ForkTTY Success Metrics

ForkTTY is alpha, so these are manual acceptance checks rather than a telemetry
plan. The matching runtime checklist is in [docs/release-qa.md](docs/release-qa.md).

## Terminal-workspace clarity

| Metric | Target | How to measure |
| --- | --- | --- |
| Time to active terminal | <= 3 seconds after window presentation | Launch with a normal saved session and time until the focused pane accepts input. |
| Workspace orientation | <= 3 seconds | Ask the tester to name the active workspace, directory, branch, and focused pane from the default view. |
| Attention discovery | <= 3 seconds | Seed one unread/needs-input notification and time how long it takes to find and open it. |
| Primary viewport noise | None | Count controls or badges unrelated to workspace, pane, focus, navigation, or attention. |

## General UI quality

| Metric | Target | How to measure |
| --- | --- | --- |
| Visual clarity | 4-5 / 5 | Review a normal laptop-width window with sidebar, terminal, pane chrome, and status bar visible. |
| Keyboard reachability | 100% of primary actions | Exercise workspace create/open, split, tab, focus, close, palette, notifications, and settings without a mouse. |
| Focus visibility | No ambiguous focus | Move between sidebar, pane tabs, terminal content, dialogs, and palette; the active target must stay obvious. |
| Contrast/readability | No obvious failures | Inspect selected rows, muted labels, warnings, disabled controls, and terminal chrome on dark and light themes. |

Use [docs/DESIGN.md](docs/DESIGN.md) as the visual source of truth: one accent,
no gradients or glow, compact native layout, sentence-case labels, and no
decorative dashboard chrome.
