# Phase 2 — Multi-Pane + Tabs: Implementation Plan

## Objective

Transform the single-terminal app into a multi-pane workspace where users can split terminals horizontally and vertically, navigate between them with keyboard shortcuts, and close individual panes. Each pane runs an independent PTY. The split layout uses a recursive binary tree rendered with `react-resizable-panels`, managed by a Zustand store that tracks the pane tree, surfaces, and focus state.

## Technical Approach

### Architecture

**State management**: A Zustand store (`src/stores/workspace.ts`) owns the pane tree and surface map. The pane tree is the recursive `PaneTree` type from SPEC.md. Each leaf references a `Surface` with its `ptyId`. The store provides actions: `splitPane`, `closePane`, `setFocusedPane`, `moveFocus`.

**Recursive rendering**: A `PaneArea` component reads the pane tree from the store and recursively renders `PanelGroup`/`Panel` (from `react-resizable-panels`) for split nodes, and `TerminalPane` for leaf nodes. Each `TerminalPane` receives its `surfaceId` as a prop and manages its own xterm.js instance + PTY lifecycle.

**PTY lifecycle**: `TerminalPane` spawns its PTY on mount and kills it on unmount. The store doesn't manage PTY lifecycle directly — it manages the tree structure, and React's mount/unmount lifecycle handles PTY creation/destruction.

**Keyboard shortcuts**: Captured at the `App` level via `useEffect` + `keydown` listener. Shortcuts dispatch store actions. Terminal input passthrough is preserved — only specific modifier combos (Ctrl+D, Ctrl+Shift+D, Alt+Arrow, Ctrl+W) are intercepted.

**Focus model**: The store tracks `focusedPaneId`. Clicking a terminal or using Alt+Arrow sets focus. The focused pane gets a visible border highlight. xterm.js `.focus()` is called when the focused pane changes.

### Data Flow

```
User presses Ctrl+D
  → App keydown handler intercepts
  → store.splitPane(focusedPaneId, 'horizontal')
  → Store updates PaneTree: leaf becomes split node with two leaf children
  → React re-renders PaneArea recursively
  → New leaf mounts → new TerminalPane → spawnPty()
  → User sees two side-by-side terminals
```

```
User presses Ctrl+W
  → store.closePane(focusedPaneId)
  → Store removes leaf from tree, simplifies parent if only one child remains
  → React unmounts removed TerminalPane → killPty()
  → Remaining panes fill the space (react-resizable-panels handles this)
  → Focus moves to sibling or parent's remaining child
```

## Tasks

### Task 1: Install dependencies
- `npm install react-resizable-panels zustand`
- Verify builds pass

### Task 2: Create Zustand workspace store (`src/stores/workspace.ts`)
- Define `PaneTree`, `Surface`, and store interfaces matching SPEC.md types
- Implement initial state: single leaf pane with a placeholder surface
- Actions: `splitPane(paneId, direction)`, `closePane(paneId)`, `setFocusedPane(paneId)`, `moveFocus(direction)`, `registerSurface(paneId, ptyId)`, `unregisterSurface(paneId)`
- `splitPane`: replaces the target leaf with a split node containing the original leaf + a new leaf
- `closePane`: removes leaf, collapses parent if it becomes single-child, moves focus to sibling
- `moveFocus`: traverses the tree to find the nearest pane in the given direction (up/down/left/right)
- Generate unique IDs with `crypto.randomUUID()`

### Task 3: Refactor `TerminalPane` to accept props
- Change from zero-prop singleton to `TerminalPane({ paneId, isFocused })`
- PTY lifecycle (spawn/write/resize/kill) remains internal to the component
- On mount: spawn PTY, register surface in store via `registerSurface(paneId, ptyId)`
- On unmount: kill PTY, unregister surface
- When `isFocused` changes to true: call `term.focus()`
- Add visual focus indicator: 2px border (e.g., `#89b4fa` blue when focused, transparent when not)
- Click handler: `store.setFocusedPane(paneId)`

### Task 4: Create `PaneArea` recursive component (`src/components/PaneArea.tsx`)
- Read pane tree from store
- For `leaf` nodes: render `<Panel><TerminalPane paneId={id} isFocused={...} /></Panel>`
- For `horizontal` splits: render `<PanelGroup direction="horizontal">` with children recursively
- For `vertical` splits: render `<PanelGroup direction="vertical">` with children
- Add `<PanelResizeHandle>` between panels (thin, styled as subtle drag handle)
- Each `Panel` needs a stable `id` prop derived from the pane ID (react-resizable-panels requirement)

### Task 5: Wire `App.tsx` to use `PaneArea` instead of direct `TerminalPane`
- Replace `<TerminalPane />` with `<PaneArea />`
- The store initializes with a single leaf, so initial render is identical to Phase 1

### Task 6: Implement keyboard shortcuts
- Add `useEffect` in `App.tsx` with `keydown` listener on `document`
- **Ctrl+D**: `store.splitPane(focusedPaneId, 'horizontal')` (split right)
- **Ctrl+Shift+D**: `store.splitPane(focusedPaneId, 'vertical')` (split down)
- **Alt+ArrowLeft/Right/Up/Down**: `store.moveFocus(direction)`
- **Ctrl+W**: `store.closePane(focusedPaneId)` — if last pane, do nothing (or close app, TBD)
- Use `e.preventDefault()` to stop default browser behavior for these combos
- NOTE: Ctrl+D in a terminal normally sends EOF. We intercept it *before* it reaches xterm.js. Users can still send EOF via the shell (typing `exit`).

### Task 7: Implement spatial navigation (`moveFocus`)
- Build a layout map from the pane tree: each leaf gets a bounding rectangle based on its position in the split hierarchy
- To move left: find the nearest pane whose right edge is to the left of the current pane's left edge
- Similar logic for right/up/down
- Simplified approach: flatten the tree to a grid-like structure, move in the requested direction
- Edge case: if no pane exists in the requested direction, do nothing (wrap is confusing)

### Task 8: Style resize handles and focus indicators
- `PanelResizeHandle`: 4px wide/tall, transparent by default, subtle highlight on hover (`#585b70`)
- Focused pane: 2px solid `#89b4fa` border
- Unfocused pane: 2px solid transparent border (prevents layout shift)
- Ensure terminals don't have gaps or overlapping borders

### Task 9: Canvas renderer per pane
- Each `TerminalPane` already loads CanvasAddon (from Phase 1)
- Verify this works with multiple panes (Canvas creates a separate `<canvas>` per terminal)
- Test with 6+ panes to ensure no context errors
- No WebGL by default (per CLAUDE.md convention)

### Task 10: Integration testing
- Build passes: `npm run build && cargo clippy && cargo test`
- Manual test matrix:
  - Launch app → single pane works (regression check)
  - Ctrl+D → splits right, both panes have independent shells
  - Ctrl+Shift+D → splits down
  - Create 2x2 grid (4 panes), each running independent shell
  - Alt+Arrow → navigates between panes, focus indicator moves
  - Ctrl+W → closes pane, remaining panes fill space
  - Close all but one → single pane remains, works normally
  - Run `htop` in one pane, `vim` in another → both render correctly
  - Resize window → all panes reflow
  - 6+ panes → no canvas/rendering errors

## Acceptance Criteria

From ROADMAP.md:
1. Split into 4 panes (2x2), each running independent shell
2. Navigate between panes with Alt+Arrow
3. Close a pane, remaining panes fill the space
4. No WebGL context errors with 6+ panes

Additional:
5. Ctrl+D splits right, Ctrl+Shift+D splits down
6. Focused pane has visible border indicator
7. Resize handles work (drag to resize splits)
8. All Phase 1 functionality preserved (single pane works, htop/vim render correctly)

## Files to Create/Modify

### New files
- `src/stores/workspace.ts` — Zustand store (pane tree, surfaces, focus, actions)
- `src/components/PaneArea.tsx` — Recursive split layout component

### Modified files
- `package.json` — Add `react-resizable-panels`, `zustand`
- `src/components/TerminalPane.tsx` — Accept `paneId`/`isFocused` props, register/unregister surface
- `src/App.tsx` — Replace `<TerminalPane />` with `<PaneArea />`, add keyboard shortcut handler
- `src/App.css` — Add styles for resize handles, focus borders

### Unchanged
- `src-tauri/src/*` — No Rust changes needed. Existing `pty_spawn`/`pty_write`/`pty_resize`/`pty_kill` already support multiple PTYs via the ID-based `PtyManager`.
- `src/lib/pty-bridge.ts` — Already supports multiple PTYs (all functions take `id` parameter)
