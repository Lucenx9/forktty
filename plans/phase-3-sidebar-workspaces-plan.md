# Phase 3 — Sidebar + Workspaces: Implementation Plan

## Objective

Add a left sidebar listing workspaces, where each workspace is a named group of panes with its own PaneTree, PTY instances, and metadata (git branch, working directory, status). Users can create, switch, rename, and close workspaces. All workspaces run simultaneously — switching is instant because inactive workspaces stay mounted but hidden.

## Technical Approach

### Multi-Workspace Architecture

The current `useWorkspaceStore` manages a single PaneTree. Phase 3 lifts this to a **map of workspaces**, each owning its own PaneTree, surfaces, and focus state.

```typescript
interface Workspace {
  id: string;
  name: string;
  root: PaneNode;
  surfaces: Record<string, Surface>;
  focusedPaneId: string;
  workingDir: string;
  gitBranch: string;
  status: 'idle' | 'running';
  createdAt: string;
}

interface WorkspaceState {
  workspaces: Record<string, Workspace>;
  activeWorkspaceId: string;
  workspaceOrder: string[];  // Controls sidebar ordering
  // ... actions
}
```

### Workspace Switching Strategy: Hide, Don't Unmount

When switching workspaces, **all workspaces remain mounted** — inactive ones get `display: none`. This keeps xterm.js instances alive, PTYs streaming, and avoids complex buffer/reconnect logic.

```tsx
{workspaceOrder.map(id => (
  <div key={id} style={{ display: id === activeId ? 'flex' : 'none', flex: 1 }}>
    <PaneArea workspaceId={id} />
  </div>
))}
```

### Git Branch Detection

Add `git2` crate to Rust backend. New Tauri command `get_git_branch(cwd: String) -> Result<String, String>` opens the repo and reads `HEAD`. Called on workspace creation and periodically (or on focus).

### Sidebar Layout

The sidebar is a fixed-width panel to the left of the pane area, separated by a draggable resize handle (react-resizable-panels). The app layout becomes:

```
<Group orientation="horizontal">
  <Panel id="sidebar" defaultSize={15} minSize={8} maxSize={30}>
    <Sidebar />
  </Panel>
  <Separator />
  <Panel id="main" defaultSize={85}>
    {/* all workspace PaneAreas, only active visible */}
  </Panel>
</Group>
```

### Status Indicator (Simplified for Phase 3)

Full OSC 133 parsing comes in Phase 5. For now, track basic activity:
- **idle** (gray): no PTY output in last 3 seconds
- **running** (green): PTY output received recently

This is tracked via a `lastActivity` timestamp on each Surface, updated when PTY data arrives.

## Tasks

### Task 1: Refactor store for multi-workspace support

Restructure `workspace.ts` to manage a map of workspaces. Each workspace owns its own `root`, `surfaces`, and `focusedPaneId`. All existing pane actions (split, close, moveFocus) operate on the **active workspace**. Export `activeWorkspace` computed getter.

**New state shape:**
- `workspaces: Record<string, Workspace>`
- `activeWorkspaceId: string`
- `workspaceOrder: string[]`

**New actions:**
- `createWorkspace(name?: string): string` — creates workspace with single leaf, returns ID
- `switchWorkspace(id: string)` — sets activeWorkspaceId
- `closeWorkspace(id: string)` — removes workspace, kills all its PTYs (via collecting leaf IDs)
- `renameWorkspace(id: string, name: string)`
- `updateSurfaceActivity(paneId: string)` — updates lastActivity timestamp

**Existing actions updated:**
- `splitPane`, `closePane`, `moveFocus`, `setFocusedPane`, `registerSurface`, `unregisterSurface` — all scoped to active workspace

**Files:** `src/stores/workspace.ts`

### Task 2: Add git2 dependency and `get_git_branch` Tauri command

Add `git2` crate. Implement `get_git_branch(cwd: String)` that:
1. Opens repo at `cwd` via `git2::Repository::discover(cwd)`
2. Reads `HEAD` reference
3. Returns branch name (shorthand) or "detached" for detached HEAD
4. Returns empty string if not a git repo (not an error)

**Files:** `src-tauri/Cargo.toml`, `src-tauri/src/lib.rs`, `src/lib/pty-bridge.ts` (add `getGitBranch` wrapper)

### Task 3: Update PaneArea to accept workspace ID

PaneArea currently reads `root` from the global store. Change it to read from a specific workspace by ID. The component receives `workspaceId` as prop and selects the workspace's root/focusedPaneId from the store.

**Files:** `src/components/PaneArea.tsx`

### Task 4: Update TerminalPane for workspace-aware surface lifecycle

TerminalPane needs to know which workspace it belongs to (for surface registration). The paneId already uniquely identifies the surface within a workspace. Update `updateSurfaceActivity` calls when PTY data arrives (for status tracking).

**Files:** `src/components/TerminalPane.tsx`

### Task 5: Create Sidebar component

New component displaying workspace list:
- Each entry shows: name (bold), git branch, working directory (truncated), status dot (colored circle)
- Active workspace has highlighted background (#89b4fa at 20% opacity)
- Click to switch workspace
- "+" button at bottom to create new workspace
- Workspace name is editable (double-click to rename)
- Right-side unread count badge (placeholder for Phase 5)

Styling: dark theme, monospace font, rounded corners on entries, subtle hover states per SPEC.md visual style.

**Files:** `src/components/Sidebar.tsx`, `src/App.css` (sidebar styles)

### Task 6: Update App.tsx layout — sidebar + multi-workspace rendering

Replace the current `<PaneArea />` with the full layout:
1. Outer `<Group orientation="horizontal">` containing sidebar panel and main panel
2. Main panel renders ALL workspaces as `<PaneArea workspaceId={id} />`, each wrapped in a div with `display: none/flex` based on active ID
3. Update keyboard shortcuts:
   - **Ctrl+N**: create new workspace
   - **Ctrl+Shift+W**: close active workspace (with confirmation if multiple panes)
   - **Ctrl+1..9**: jump to workspace by position

**Files:** `src/App.tsx`, `src/App.css`

### Task 7: Wire git branch detection

On workspace creation, call `getGitBranch(cwd)` to populate the workspace's `gitBranch` field. Also refresh on workspace focus (switch). Store the result in the workspace state.

The CWD for now is the app's working directory (process.cwd equivalent — use Tauri's `resolveResource` or just pass from Rust). In Phase 4, each workspace will have its own worktree CWD.

**Files:** `src/stores/workspace.ts` (add async init), `src/components/Sidebar.tsx`

### Task 8: Surface activity tracking for status dots

When PTY data arrives in TerminalPane, call `updateSurfaceActivity(paneId)` in the store. The store sets `lastActivity = Date.now()` on the surface.

Sidebar reads `lastActivity` for all surfaces in a workspace and shows:
- Green dot if any surface has activity within last 3 seconds
- Gray dot otherwise

Use a 1-second interval in Sidebar to re-evaluate status (or derive from Zustand subscription).

**Files:** `src/components/TerminalPane.tsx`, `src/stores/workspace.ts`, `src/components/Sidebar.tsx`

### Task 9: Workspace auto-naming

When creating a workspace without an explicit name, auto-generate names: "Workspace 1", "Workspace 2", etc. (incrementing counter, skipping existing names).

The first workspace (created on app start) is named "Workspace 1".

**Files:** `src/stores/workspace.ts`

### Task 10: Verification and testing

Run all verification gates:
- `cargo clippy --manifest-path src-tauri/Cargo.toml -- -W clippy::all`
- `cargo fmt --manifest-path src-tauri/Cargo.toml --check`
- `npm run build`
- `npx prettier --check src/`

Manual test checklist:
- App launches with sidebar showing "Workspace 1"
- Ctrl+N creates "Workspace 2" and switches to it
- Click workspace entries to switch — pane layouts preserved
- Git branch shows in sidebar entries
- Status dots update (green when terminal active, gray when idle)
- Ctrl+1/Ctrl+2 jumps between workspaces
- Ctrl+Shift+W closes a workspace, PTYs are killed
- Sidebar resize handle works
- Splitting/closing panes within a workspace still works
- All shortcuts from Phase 2 still work

## Acceptance Criteria (from ROADMAP.md)

- [ ] Create 3 workspaces, switch between them, each preserves its own pane layout
- [ ] Sidebar shows branch name and working directory per workspace
- [ ] Close a workspace, its PTYs are killed

## Additional Acceptance

- [ ] Sidebar is resizable via drag handle
- [ ] Ctrl+N creates new workspace, Ctrl+Shift+W closes it
- [ ] Ctrl+1..9 jumps to workspace by position
- [ ] Status dots show idle/running state
- [ ] Workspace rename via double-click works
- [ ] No regressions: Phase 2 split/navigate/close still works

## Files to Create/Modify

| File | Action | Description |
|------|--------|-------------|
| `src/stores/workspace.ts` | **Modify** | Restructure for multi-workspace support |
| `src/components/Sidebar.tsx` | **Create** | Workspace list with metadata and actions |
| `src/components/PaneArea.tsx` | **Modify** | Accept workspaceId prop |
| `src/components/TerminalPane.tsx` | **Modify** | Activity tracking, workspace-aware registration |
| `src/App.tsx` | **Modify** | Sidebar layout, multi-workspace rendering, new shortcuts |
| `src/App.css` | **Modify** | Sidebar styles, workspace entry styles |
| `src/lib/pty-bridge.ts` | **Modify** | Add getGitBranch wrapper |
| `src-tauri/Cargo.toml` | **Modify** | Add git2 dependency |
| `src-tauri/src/lib.rs` | **Modify** | Add get_git_branch command |

## Dependencies

- No new npm packages needed (react-resizable-panels already installed for sidebar resize)
- New Rust crate: `git2` for branch detection
- No changes to Tauri config needed
