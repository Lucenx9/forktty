# Desktop productivity UI research: Linear, Cursor, and Raycast

**Date:** 2026-07-17
**Scope:** First-party patterns relevant to a dense, keyboard-oriented desktop productivity app.
**Products:** Linear, Cursor, Raycast.
**Topics:** hierarchy and density; keyboard navigation and command surfaces; focus; feedback; empty states and onboarding; accessibility; motion; menus and settings.

## 1. Method and evidence limits

This report uses only official product documentation, changelogs, manuals, developer documentation, and company design posts. It intentionally excludes reviews, screenshots posted by third parties, and generalized claims such as “Linear-like” or “Raycast-like” unless an official source describes the behavior.

Evidence is graded as follows:

- **A — directly retrieved:** the official page content was retrieved and the stated behavior could be checked directly.
- **B — official source, index-verified:** an official page and its first-party summary were found, but the page itself could not be retrieved during this run. These claims are kept narrow and paraphrased.
- **Not established:** no sufficiently specific first-party evidence was found. Absence of documentation is not evidence that a product lacks the behavior.

The strongest direct evidence in this pass came from Raycast's manual and extension API. Linear's official design posts and Cursor's official docs/changelogs were index-verified but intermittently unavailable to the fetcher. Consequently, Raycast supports more detailed claims below; that asymmetry reflects source quality, not a judgment that Raycast has better UX.

## 2. Sourced facts

This section reports product behavior or principles stated by the companies. Recommendations do not begin until section 3.

### 2.1 Linear

#### Hierarchy and density

- **L1 (B):** Linear's 2024 redesign explicitly addresses interface density, visual hierarchy, light/dark appearance, contrast controls, and high-contrast themes. Source: [How we redesigned the Linear UI (part II)](https://linear.app/now/how-we-redesigned-the-linear-ui) (2024-03-28).
- **L2 (B):** Linear's 2026 refresh describes a compact navigation treatment, reduced visual noise, revised icon sizing, color controls, and greater interface consistency while retaining information density. Source: [A calmer interface for a product in motion](https://linear.app/now/behind-the-latest-design-refresh) (2026-03-12).
- **L3 (B):** Linear frames its earlier redesign as a response to accumulated design debt as the product expanded. This establishes coherence across a growing product—not minimalism by itself—as the redesign problem. Source: [A design reset (part I)](https://linear.app/blog/a-design-reset) (2024-03-27).

#### Accessibility and settings

- **L4 (B):** Linear's redesign documentation treats light/dark modes, user-controlled contrast, and high-contrast themes as part of the interface system rather than a separate skin. Source: [How we redesigned the Linear UI (part II)](https://linear.app/now/how-we-redesigned-the-linear-ui).
- **L5 (B):** The 2026 refresh includes explicit color controls and consistency work. Source: [A calmer interface for a product in motion](https://linear.app/now/behind-the-latest-design-refresh).

#### Evidence not established in this pass

- A detailed, directly retrieved description of Linear's command menu, focus restoration, menu ordering, empty-state rules, screen-reader support, or reduced-motion behavior.
- Linear is widely associated with keyboard shortcuts, but this report does not turn that reputation into detailed factual claims without a successfully retrieved first-party page.

### 2.2 Cursor

#### Keyboard-first operation and focus

- **C1 (B):** Cursor documents remappable keybindings, shortcuts for settings and chat navigation, and `Esc` behavior that removes focus from a field. Source: [Keyboard Shortcuts — Cursor Docs](https://docs.cursor.com/advanced/keyboard-shortcuts).
- **C2 (B):** Cursor documents shortcut customization under **Settings → Keyboard Shortcuts**. Its official changelog also calls out high-contrast themes and UI improvements. Source: [Reliability, Keyboard Shortcuts & Early Access Opt-In](https://www.cursor.com/en/changelog/reliability-keyboard-shortcuts-early-access-opt-in) (2025-03-11).
- **C3 (B):** Cursor's newer interface documentation attaches keyboard shortcuts to the Agents Window and Design Mode rather than making those surfaces pointer-only. Source: [New Cursor Interface](https://cursor.com/changelog/3-0).
- **C4 (B):** Cursor's 2026 full-screen-tab and compact-chat changes include display-density settings, keyboard navigation, and fixes for shortcut and caret behavior. Source: [Full-Screen Tabs and Compact Chats](https://cursor.com/changelog/3-4) (2026-05-13).
- **C5 (B):** Cursor exposes an explicit terminal **Focus** control in the product and documents it in the changelog. Source: [Shared Terminal with Agent, Context Usage, and Faster Edits](https://cursor.com/changelog/1-3) (2025-07-29).

#### Onboarding and settings

- **C6 (B):** Cursor's first-run setup asks users to choose familiar shortcuts, a theme, and terminal preferences, and the setup wizard can be reopened. Source: [Installation and First-Time Setup — Cursor Docs](https://docs.cursor.com/get-started/installation).
- **C7 (B):** Cursor has documented preference import, theme selection, custom keybindings, and mode settings as onboarding/configuration concerns. Source: [Chat Tabs, Custom Modes & Faster Indexing](https://cursor.com/changelog/0-48-x) (2025-03-23).
- **C8 (B):** Cursor previously improved first-run import on Windows and restoration of folder/window state. Source: [AI Previews Beta](https://cursor.com/changelog/0-26) (2024-02-09).

#### Hierarchy, density, and feedback

- **C9 (B):** Full-screen tabs and compact chats are explicit alternatives for changing visual focus and density, rather than one fixed layout. Source: [Full-Screen Tabs and Compact Chats](https://cursor.com/changelog/3-4).
- **C10 (B):** Cursor documents checkpoint support in notebook workflows. Source: [Shared Terminal with Agent, Context Usage, and Faster Edits](https://cursor.com/changelog/1-3). The available evidence establishes the feature's presence, but not a complete first-party feedback taxonomy.

#### Evidence not established in this pass

- A dedicated Cursor accessibility guide covering screen readers, complete keyboard traversal, contrast requirements, or reduced motion.
- A company-authored design rationale equivalent to Linear's redesign essays.
- A complete rule for Agent progress, success/error feedback, empty states, or restoration of focus after every overlay.
- Accessibility parity with upstream VS Code. Cursor's ancestry is not sufficient evidence of behavioral parity and should not be assumed.

### 2.3 Raycast

#### Keyboard-first navigation and command surfaces

- **R1 (A):** Raycast's manual states that it is “built to be driven entirely from the keyboard.” Its global shortcut opens or closes Raycast; arrow keys navigate lists; `Enter` executes the primary action; and `⌘/Ctrl K` opens the Action Panel. Source: [Keyboard Shortcuts — Raycast Manual](https://manual.raycast.com/keyboard-shortcuts).
- **R2 (A):** The Action Panel is itself searchable by typing. `Enter` runs the selected action, while modifier combinations address secondary and tertiary actions. `Esc` closes the panel or backs out of a submenu. Source: [Keyboard Shortcuts — Raycast Manual](https://manual.raycast.com/keyboard-shortcuts).
- **R3 (A):** Raycast supports alternative keyboard models, including enabled-by-default Emacs navigation and optional Vim bindings. Navigation bindings are configurable under **Settings → Keyboard**. Source: [Keyboard Shortcuts — Raycast Manual](https://manual.raycast.com/keyboard-shortcuts).
- **R4 (A):** Raycast's developer API recommends assigning shortcuts to frequently used actions. Actions expose a title, icon, and optional shortcut; common built-in actions are provided to keep behavior consistent. Source: [Actions — Raycast API](https://developers.raycast.com/api-reference/user-interface/actions).
- **R5 (B):** Raycast's user manual describes the Action Panel as the home for contextual actions, search, organization, submenus, and shortcut configuration. Source: [Action Panel — Raycast Manual](https://manual.raycast.com/action-panel).

#### Focus and navigation behavior

- **R6 (A):** `Esc` has layered behavior: it returns to the preceding view, closes Raycast from Root Search, closes an Action Panel, backs out of a submenu, or cancels a form and returns. Source: [Keyboard Shortcuts — Raycast Manual](https://manual.raycast.com/keyboard-shortcuts).
- **R7 (A):** An Action Panel or submenu can designate which action receives focus when it opens. Pushed views participate in a navigation stack with push/pop lifecycle hooks. Source: [Actions — Raycast API](https://developers.raycast.com/api-reference/user-interface/actions).
- **R8 (A):** Raycast lists maintain an explicit selected item. Filtering can leave no selection, represented as `null`, and details/actions follow the selected row. Source: [List — Raycast API](https://developers.raycast.com/api-reference/user-interface/list).

#### Hierarchy and density

- **R9 (A):** Raycast's canonical list structure uses items and optional sections. The title is primary; subtitles, accessories, and an optional detail pane carry secondary information. Source: [List — Raycast API](https://developers.raycast.com/api-reference/user-interface/list).
- **R10 (A):** When a detail pane is visible, Raycast recommends omitting row accessories and moving supplementary information into the detail area. This avoids showing the same secondary information twice. Source: [List — Raycast API](https://developers.raycast.com/api-reference/user-interface/list).
- **R11 (A):** Sections organize related entities; metadata separators and tag lists create compact grouping in the detail pane. Source: [List — Raycast API](https://developers.raycast.com/api-reference/user-interface/list).
- **R12 (B):** Raycast documents interface sizing, appearance, global hotkeys, navigation options, and extension configuration in one settings surface. Source: [Settings — Raycast Manual](https://manual.raycast.com/settings).
- **R13 (B):** Raycast's Search Bar documentation includes Compact Mode, empty-query behavior, keyboard navigation, and Action Panel access. Source: [Search Bar — Raycast Manual](https://manual.raycast.com/search-bar).

#### Feedback

- **R14 (A):** Raycast's list API can show a loading bar below the search field, and its guidance says to show a loading indicator for longer operations. Loading is attached to the region doing the work: list, dropdown, or detail. Source: [List — Raycast API](https://developers.raycast.com/api-reference/user-interface/list).
- **R15 (A):** For asynchronous work, a toast can begin in an animated/loading state and remain until the process completes. The same toast can then be updated to success or failure instead of producing unrelated messages. Source: [Toast — Raycast API](https://developers.raycast.com/api-reference/feedback/toast).
- **R16 (A):** Failure toasts can carry a short title plus diagnostic detail. Toasts may expose primary and secondary actions such as cancel, undo, retry, or copying diagnostics. Source: [Toast — Raycast API](https://developers.raycast.com/api-reference/feedback/toast).
- **R17 (A):** When the Raycast window is closed, toast feedback falls back to a HUD. Source: [Toast — Raycast API](https://developers.raycast.com/api-reference/feedback/toast).

#### Empty and loading states

- **R18 (A):** A list with no data or no query matches receives a default empty state. A custom `EmptyView` can provide a title, description, icon, and actions. Source: [List — Raycast API](https://developers.raycast.com/api-reference/user-interface/list).
- **R19 (A):** The empty state is suppressed while an initially blank list is loading, avoiding a false “empty” message before data arrives. Source: [List — Raycast API](https://developers.raycast.com/api-reference/user-interface/list).

#### Menus and destructive actions

- **R20 (A):** Raycast distinguishes regular and destructive actions. Its API guidance says irreversible actions should use a confirmation alert. Source: [Actions — Raycast API](https://developers.raycast.com/api-reference/user-interface/actions).
- **R21 (A):** Submenus are supported for actions such as choosing an application to open a file or URL. Source: [Actions — Raycast API](https://developers.raycast.com/api-reference/user-interface/actions).

#### Accessibility and motion evidence limits

- **R22 (A):** Raycast's documented keyboard model is extensive, required labels provide visible names for core list/action elements, and tooltips are supported. Source: [Keyboard Shortcuts — Raycast Manual](https://manual.raycast.com/keyboard-shortcuts) and [List — Raycast API](https://developers.raycast.com/api-reference/user-interface/list).
- **Not established:** the retrieved API pages do not state screen-reader semantics, contrast thresholds, focus-ring requirements, or reduced-motion behavior. An official search also did not surface a dedicated reduced-motion page. These must be tested separately rather than inferred from keyboard support.

## 3. Cross-product synthesis

Everything in this section is a derived recommendation, not a claim that all three products implement the same pattern.

### 3.1 Density is compression with hierarchy, not indiscriminate shrinking

**Basis:** Linear explicitly discusses density, reduced noise, icon sizing, and coherent hierarchy (L1–L3). Cursor offers user-selectable compact and focus-oriented layouts (C4, C9). Raycast assigns different information roles to titles, subtitles, accessories, sections, and details, and advises removing accessories when details are visible (R9–R11).

**Synthesis:**

- Keep the primary work surface visually dominant.
- Compress repeated chrome before compressing meaningful content.
- Give primary identity, secondary context, status, and actions distinct placements.
- Do not duplicate secondary metadata in both a row/header and a visible detail area.
- Treat compactness as a mode or controlled system where user needs differ; do not assume one density fits every task.

### 3.2 “Keyboard-first” requires both reachability and discoverability

**Basis:** Cursor exposes remappable keybindings and documents focus/navigation commands (C1–C4). Raycast exposes a global launcher, searchable Action Panel, visible action shortcuts, alternate navigation models, and configurable bindings (R1–R5).

**Synthesis:**

A keyboard-first interface needs all four of these layers:

1. A predictable global entry point.
2. Keyboard traversal within the current context.
3. A searchable action surface for commands the user has not memorized.
4. Visible shortcut labels and remapping so expertise can grow over time.

A command palette alone does not satisfy the requirement if users cannot discover it, if it loses context, or if essential actions remain pointer-only.

### 3.3 Focus behavior should be modeled as navigation state

**Basis:** Cursor documents explicit focus/unfocus commands and has fixed caret/shortcut behavior (C1, C4, C5). Raycast documents selected-row state, initial action focus, navigation stacks, and layered `Esc` behavior (R6–R8).

**Synthesis:**

- Every overlay or nested surface should define initial focus, keyboard traversal, `Esc`, successful completion, cancellation, and return focus.
- `Esc` should unwind one layer at a time instead of unpredictably closing several contexts.
- Opening a contextual action surface should preserve the selected object.
- Closing it should return focus to the invoking control or work surface.
- No-match filtering is a real focus state, not an exceptional condition.

### 3.4 Feedback should match duration, locality, and consequence

**Basis:** Raycast differentiates local loading indicators, mutable progress/success/failure toasts, actionable error feedback, and out-of-window HUDs (R14–R17). Cursor's checkpoint and state-restoration features establish undo/recovery as relevant feedback mechanisms, though the available source is less detailed (C8, C10).

**Synthesis:**

- Immediate local changes should update in place.
- Longer work should show progress where the work is occurring.
- Background work may use a persistent, updatable notification.
- Completion should replace progress rather than stack another unrelated message.
- Errors should state what failed and expose the most useful recovery action.
- Destructive or broad changes need confirmation or a credible undo/recovery path.

### 3.5 Empty states are task states, not decoration

**Basis:** Raycast distinguishes loading from empty/no-match states and permits an explanation plus actions (R18–R19). Cursor's first-run flow asks users for familiar defaults and supports reopening setup (C6–C8).

**Synthesis:**

- Distinguish “loading,” “no objects exist,” “no results match,” and “an error prevented loading.”
- Empty states should state what the region contains and offer the next useful action.
- First-run setup should ask only for choices that materially improve immediate use.
- Onboarding must be recoverable: users should be able to reopen setup or change those choices later.

### 3.6 Accessibility cannot be inferred from visual polish or keyboard marketing

**Basis:** Linear documents contrast controls and high-contrast themes (L4–L5). Cursor documents remapping and high-contrast themes (C1–C2). Raycast documents extensive keyboard operation (R1–R3), but the retrieved sources do not establish screen-reader or reduced-motion behavior.

**Synthesis:**

Keyboard access, visible focus, contrast, screen-reader semantics, target sizing, and reduced motion are separate audit dimensions. Passing one does not imply passing the others. In particular:

- Do not infer screen-reader quality from shortcut coverage.
- Do not infer Cursor's accessibility behavior from VS Code ancestry.
- Do not infer reduced-motion support from restrained-looking animation.
- Avoid hover-only access to essential actions even when the app targets expert users.

### 3.7 Motion is an evidence gap, so use it conservatively

The reviewed first-party evidence is strong on hierarchy, shortcuts, state, and feedback but weak on explicit motion principles and reduced-motion support. Therefore no “Linear motion language,” “Cursor animation system,” or “Raycast spring” should be treated as a sourced benchmark.

**Synthesis:** motion should explain continuity or state change, remain interruptible, avoid delaying input, and have a reduced/non-animated path. This is a conservative audit stance, not a documented cross-product consensus.

### 3.8 Menus and settings should expose an action model, not become storage rooms

**Basis:** Raycast makes contextual actions searchable, shortcut-labeled, nestable, and semantically destructive where needed (R4–R5, R20–R21). Cursor centralizes keybinding customization and lets users revisit first-run choices (C2, C6–C7). Linear treats appearance controls as part of a coherent visual system (L4–L5).

**Synthesis:**

- Put object-specific actions near the object's context, with the primary action first.
- Make the same action names searchable from a broader command surface where practical.
- Show shortcuts beside commands rather than documenting them only elsewhere.
- Group settings by user goal; make consequential settings legible and reversible.
- Keep common defaults strong, while preserving remapping and advanced configuration for expert workflows.

## 4. Concrete audit criteria

The checklist below is intentionally observable. “Pass” should be demonstrable with keyboard-only use and, for accessibility criteria, with the relevant assistive settings or technology.

### 4.1 Hierarchy and density

| ID | Audit test | Pass criterion | Evidence basis |
|---|---|---|---|
| HD-1 | Inspect the default window at normal size. | One primary work surface is unambiguous; navigation, context, and status are subordinate. | L1–L3, C9, R9 |
| HD-2 | Compare every persistent label, badge, divider, and toolbar control with the task it supports. | Persistent chrome is removed, merged, or progressively disclosed when it does not affect the current task. | L2, R9–R11 |
| HD-3 | Open a row/pane together with its detail or inspector. | Secondary metadata is not duplicated in both places unless repetition prevents a real error. | R10 |
| HD-4 | Scan a dense list without opening items. | Primary identity, secondary context, status, and actions use consistent positions and visual weights. | L1–L2, R9–R11 |
| HD-5 | Resize to a narrow but supported width. | Essential identity and state survive before secondary metadata and optional actions. | Synthesis from L1–L2, C4, R9–R10 |
| HD-6 | Switch between ordinary and concentration-heavy work. | The product offers a predictable way to hide or compact nonessential regions without losing state. | C4, C9, R13 |

### 4.2 Keyboard navigation and command surfaces

| ID | Audit test | Pass criterion | Evidence basis |
|---|---|---|---|
| KB-1 | Start from an arbitrary work surface and use only the keyboard. | A documented, stable shortcut opens the global command/search surface. | C1–C3, R1 |
| KB-2 | Search for an action whose shortcut is unknown. | Search matches recognizable action names and shows the action's shortcut when one exists. | R2, R4–R5 |
| KB-3 | Navigate lists, panes, overlays, and settings without a pointer. | All core tasks are reachable; selection and focus remain visible. | C1–C5, R1–R3, R6–R8 |
| KB-4 | Inspect contextual menus and command results. | Frequent actions have learnable shortcuts; essential actions are not available only on hover or right-click. | R4–R5; synthesis for hover |
| KB-5 | Change a conflicting shortcut. | Keybindings can be inspected and remapped, and conflicts are reported rather than silently ignored. | C1–C2, R3–R5 |
| KB-6 | Use the product with its documented alternate navigation model, if offered. | Alternate bindings do not make labels, help, or focus behavior misleading. | R3 |

### 4.3 Focus behavior

| ID | Audit test | Pass criterion | Evidence basis |
|---|---|---|---|
| FC-1 | Open each palette, dialog, submenu, and inspector by keyboard. | Initial focus lands on the expected search field, selected action, or first meaningful control. | C1, R7 |
| FC-2 | Press `Esc` from nested UI. | Exactly one layer closes or navigates back; focus returns to the prior meaningful context. | C1, R6–R7 |
| FC-3 | Open contextual actions for a selected object. | The object remains selected and the action surface operates on that object. | R7–R8 |
| FC-4 | Filter a list until no item matches, then clear the query. | No-match is handled without phantom selection; a valid selection is restored predictably when results return. | R8, R18–R19 |
| FC-5 | Move between editor/terminal/work surface and side panels. | Focus has an explicit keyboard path and never disappears into an unfocusable region. | C1, C4–C5; synthesis |
| FC-6 | Complete and cancel an overlay action. | Both paths have defined destinations and do not steal focus into unrelated panes. | R6–R8; synthesis |

### 4.4 Feedback and recovery

| ID | Audit test | Pass criterion | Evidence basis |
|---|---|---|---|
| FB-1 | Trigger an operation that takes perceptible time. | Progress appears promptly in the affected region or in a persistent status message. | R14–R15 |
| FB-2 | Let the operation succeed. | The existing progress state resolves to success instead of producing a disconnected stack of messages. | R15 |
| FB-3 | Force the operation to fail. | The error names the failed operation, preserves useful detail, and offers the best available recovery action. | R16 |
| FB-4 | Start cancellable or reversible work. | Cancel, undo, retry, or diagnostic-copy actions remain available for as long as they are useful. | R16, C8, C10 |
| FB-5 | Start work and move focus or close the initiating surface. | Important background completion/failure remains visible without forcibly stealing focus. | R17; synthesis |
| FB-6 | Invoke an irreversible action. | It is visually identified as destructive and requires confirmation unless a reliable undo makes confirmation unnecessary. | R20; synthesis for undo exception |

### 4.5 Empty states and onboarding

| ID | Audit test | Pass criterion | Evidence basis |
|---|---|---|---|
| EO-1 | Open a region with no data. | The empty state names the region's purpose and provides a relevant primary action when one exists. | R18 |
| EO-2 | Search for a query with no matches. | “No matches” is distinct from “no data,” loading, and error. | R18–R19 |
| EO-3 | Observe initial loading. | An empty message does not flash before the first load completes. | R14, R19 |
| EO-4 | Complete first run. | Setup asks only for high-value defaults such as familiar keybindings, theme, or terminal behavior. | C6–C7 |
| EO-5 | Change your mind after first run. | Setup can be reopened or every onboarding choice can be changed in ordinary settings. | C6–C8 |
| EO-6 | Resume after restart or import. | Restorable workspace/window state is preserved where safe and understandable. | C8; synthesis |

### 4.6 Accessibility

| ID | Audit test | Pass criterion | Evidence basis |
|---|---|---|---|
| AX-1 | Complete every core workflow with keyboard only. | No essential action requires pointer hover, drag, or an unlabeled gesture. | C1–C5, R1–R5 |
| AX-2 | Traverse controls and selected rows. | Focus and selection are visible, distinct, and not conveyed by color alone. | L4–L5, C2; synthesis |
| AX-3 | Enable a high-contrast theme or equivalent appearance setting. | Text, selected state, focus, errors, and disabled state remain distinguishable. | L4–L5, C2 |
| AX-4 | Inspect controls with a screen reader. | Controls, panes, lists, status changes, and destructive actions have meaningful names, roles, states, and ordering. | Required independent test; not established by product evidence |
| AX-5 | Enable reduced motion. | Nonessential transitions stop or simplify; no task depends on animation to communicate state. | Conservative synthesis; motion evidence gap |
| AX-6 | Increase text/UI scale and use a narrow window. | Content reflows or truncates intelligibly without hiding the current selection or primary action. | Synthesis from density/appearance evidence |
| AX-7 | Review shortcuts for conflicts and motor accessibility. | Bindings are discoverable and remappable, and alternative navigation does not remove ordinary access. | C1–C2, R3–R5 |

### 4.7 Motion

| ID | Audit test | Pass criterion | Evidence basis |
|---|---|---|---|
| MO-1 | Trigger every transition repeatedly while entering input. | Motion never blocks input, delays focus, or changes the target during activation. | Conservative synthesis; explicit source gap |
| MO-2 | Open and close nested surfaces. | Motion clarifies continuity and layer direction; the same hierarchy remains understandable without it. | Conservative synthesis |
| MO-3 | Enable reduced motion. | Transforms and large spatial transitions are removed or shortened while state feedback remains complete. | Conservative synthesis |
| MO-4 | Trigger progress, success, and failure. | Animation is used only while state is genuinely indeterminate; it resolves to a stable terminal state. | R15 |

### 4.8 Menus and settings

| ID | Audit test | Pass criterion | Evidence basis |
|---|---|---|---|
| MS-1 | Open contextual actions for several object types. | The primary action is first/focused; related actions are grouped; destructive actions are separated and identified. | R4–R5, R7, R20–R21 |
| MS-2 | Type in the action surface. | Action names are searchable, including actions nested in a manageable submenu. | R2, R5, R21 |
| MS-3 | Compare palette, context menu, and settings terminology. | The same action or setting uses the same searchable name across surfaces. | L3, R4–R5; synthesis |
| MS-4 | Open settings from the keyboard. | Settings expose appearance, keyboard, navigation, and feature configuration in stable, goal-oriented groups. | C2, C6–C7, R3, R12 |
| MS-5 | Inspect current shortcuts in settings and menus. | Current bindings—not merely defaults—are visible where actions are discovered. | C1–C2, R3–R5 |
| MS-6 | Change a consequential preference. | The effect is previewable or reversible, and reset/default behavior is clear. | L4–L5, C6–C8; synthesis |

## 5. Priority order for a desktop-productivity audit

If time is limited, audit in this order:

1. **Keyboard reachability and visible focus:** failures block expert and accessibility workflows simultaneously.
2. **Focus lifecycle:** test open, navigate, execute, cancel, and return for every overlay or nested surface.
3. **Feedback and recovery:** especially long-running, background, destructive, and failure paths.
4. **Hierarchy and duplicate chrome:** remove repeated metadata and protect the primary work surface.
5. **Loading, empty, no-match, and error differentiation.**
6. **Shortcut discovery/remapping and action naming consistency.**
7. **High contrast, screen reader, scaling, and reduced-motion tests:** do not mark accessibility complete based on keyboard coverage alone.
8. **Motion polish:** only after state, focus, and reduced-motion behavior are correct.

## 6. Findings to avoid overgeneralizing

- Linear's official material supports deliberate density, lower visual noise, coherent hierarchy, and contrast controls. It does **not**, from the evidence retained here, justify copying arbitrary spacing, animation timing, or a particular command-menu implementation.
- Cursor supports multiple density/focus configurations, remappable shortcuts, explicit focus controls, and recoverable onboarding choices. Its VS Code foundation should **not** be used as proof of accessibility parity.
- Raycast provides the clearest documented action, focus, empty-state, and feedback model because those patterns are exposed as a public extension API. That model fits command/list workflows especially well; it should not be imposed unchanged on an editor or terminal workspace.
- Across the products, the robust shared idea is not “make it look minimal.” It is: preserve a legible hierarchy, make expert paths fast and discoverable, define focus transitions, expose progress and recovery, and let configuration serve the workflow without dominating it.

## 7. Official source index

### Linear

- [A design reset (part I)](https://linear.app/blog/a-design-reset)
- [How we redesigned the Linear UI (part II)](https://linear.app/now/how-we-redesigned-the-linear-ui)
- [A calmer interface for a product in motion](https://linear.app/now/behind-the-latest-design-refresh)

### Cursor

- [Keyboard Shortcuts — Cursor Docs](https://docs.cursor.com/advanced/keyboard-shortcuts)
- [Installation and First-Time Setup — Cursor Docs](https://docs.cursor.com/get-started/installation)
- [Reliability, Keyboard Shortcuts & Early Access Opt-In](https://www.cursor.com/en/changelog/reliability-keyboard-shortcuts-early-access-opt-in)
- [Chat Tabs, Custom Modes & Faster Indexing](https://cursor.com/changelog/0-48-x)
- [Shared Terminal with Agent, Context Usage, and Faster Edits](https://cursor.com/changelog/1-3)
- [New Cursor Interface](https://cursor.com/changelog/3-0)
- [Full-Screen Tabs and Compact Chats](https://cursor.com/changelog/3-4)
- [AI Previews Beta](https://cursor.com/changelog/0-26)

### Raycast

- [Keyboard Shortcuts — Raycast Manual](https://manual.raycast.com/keyboard-shortcuts)
- [Action Panel — Raycast Manual](https://manual.raycast.com/action-panel)
- [Search Bar — Raycast Manual](https://manual.raycast.com/search-bar)
- [Settings — Raycast Manual](https://manual.raycast.com/settings)
- [Actions — Raycast API](https://developers.raycast.com/api-reference/user-interface/actions)
- [List — Raycast API](https://developers.raycast.com/api-reference/user-interface/list)
- [Toast — Raycast API](https://developers.raycast.com/api-reference/feedback/toast)
