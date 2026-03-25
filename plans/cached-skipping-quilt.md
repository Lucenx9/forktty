# ForkTTY Feature Parity Plan

## Context

ForkTTY is missing several features compared to cmux (the reference terminal multiplexer for AI agents). The user wants ALL missing functionality implemented. This plan covers 9 features + ROADMAP cleanup, organized in dependency order.

## Implementation Order

### Step 1: OSC 9/99/777 Parsing (output_scanner.rs)

Extend the output scanner to detect notification escape sequences beyond OSC 133.

**Files:**
- `src-tauri/src/output_scanner.rs` — Refactor `scan_osc133()` into a general `scan_osc()` that dispatches on prefix: `133;` (existing), `9;`, `99;`, `777;notify;`
- `src/lib/pty-bridge.ts` — Add `ScanEventNotification` type to `ScanEventData` union
- `src/components/TerminalPane.tsx` — Handle `event_type === "notification"` in `handleScanEvent`

**New ScanEvent variant:**
```rust
Notification { title: String, body: String }
```

**OSC formats:**
- OSC 9: `\x1b]9;<text>\x07` → `Notification { title: "Terminal", body: text }`
- OSC 99: `\x1b]99;<id>;<text>\x07` → `Notification { title: "Terminal", body: text }`
- OSC 777: `\x1b]777;notify;<title>;<body>\x07` → `Notification { title, body }`

**Performance:** Single scan for `\x1b]` then dispatch on suffix. No allocations unless OSC found. Reuse existing `osc_buf` for split-across-chunks.

**Tests:** Unit tests for each OSC type + split-across-chunks + OSC 133 regression.

---

### Step 2: Notification Ring on Terminal Panes

Blue glowing border on panes with unread notifications.

**Files:**
- `src/stores/workspace.ts` — Add `hasUnreadNotification: boolean` to `Surface` interface. Add `setSurfaceUnread(surfaceId, unread)` action. Clear in `setFocusedPane`.
- `src/components/TerminalPane.tsx` — Read `hasUnreadNotification` from store. Set true in `handleScanEvent` when notification fires. Apply CSS class `pane-notification-ring` when true and not focused.
- `src/App.css` — Add `@keyframes notification-pulse` animation + `.pane-notification-ring` class (blue border + shadow glow, 2s pulse loop)

**Visual priority:** focused > notification ring > transparent border.

---

### Step 3: Sidebar Notification Preview

Show latest notification text inline in sidebar workspace entries.

**Files:**
- `src/stores/workspace.ts` — Add `lastNotificationText: string` to `Workspace`. Set in `addNotification()`. Clear in `markWorkspaceRead()`.
- `src/components/Sidebar.tsx` — Render preview below working dir when `lastNotificationText` is set and workspace is not active.
- `src/App.css` — `.sidebar-notification-preview` style (blue, italic, 10px, ellipsis)

---

### Step 4: Window Title Badge

Show unread count in window title.

**Files:**
- `src/App.tsx` — Add `useEffect` with Zustand selector for total unread count across all workspaces. Set `document.title = totalUnread > 0 ? \`ForkTTY (${totalUnread})\` : "ForkTTY"`.

---

### Step 5: Auto-Reorder on Notification

Move workspace to top of sidebar when it receives a notification.

**Files:**
- `src-tauri/src/config.rs` — Add `auto_reorder: bool` (default true) to `NotificationConfig`
- `src/stores/workspace.ts` — In `addNotification()`, if config `auto_reorder` enabled and workspace is not active, move `workspaceId` to front of `workspaceOrder`
- `src/components/SettingsPanel.tsx` — Add checkbox for auto-reorder

---

### Step 6: read-screen Command

Dump terminal buffer content via socket API + CLI.

**New file:**
- `src/lib/terminal-registry.ts` — Map<string, Terminal> registry with `registerTerminal()`, `unregisterTerminal()`, `readScreen()` (iterates `terminal.buffer.active` lines)

**Files:**
- `src/components/TerminalPane.tsx` — Call `registerTerminal(paneId, term)` after `term.open()`, `unregisterTerminal(paneId)` before `term.dispose()`
- `src/App.tsx` — Add `surface.read_screen` case to `handleSocketRequest`, import `readScreen` from registry
- `src-tauri/src/socket_api.rs` — Add `"surface.read_screen"` to bridged methods list

---

### Step 7: Flash Focused Pane

Brief blue flash on pane focus change.

**Files:**
- `src/components/TerminalPane.tsx` — Add `flashBorder` state. `useEffect` on `isFocused`: set true, setTimeout 500ms to false. Apply `.pane-focus-flash` class.
- `src/App.css` — `@keyframes focus-flash` (strong glow → none over 500ms) + `.pane-focus-flash` class

---

### Step 8: Workspace Drag-and-Drop Reorder

Drag sidebar entries to reorder.

**Files:**
- `src/stores/workspace.ts` — Add `reorderWorkspaces(fromIndex, toIndex)` action (splice + insert in `workspaceOrder`)
- `src/components/Sidebar.tsx` — HTML5 DnD: `draggable`, `onDragStart` (set data), `onDragOver` (preventDefault + compute position), `onDrop` (call reorder). Track `dragOverIndex` state for drop indicator.
- `src/App.css` — `.sidebar-entry-dragging` (opacity 0.5) + `.sidebar-drop-indicator` (2px blue line)

---

### Step 9: System Tray

Tray icon with tooltip showing unread count.

**Files:**
- `src-tauri/Cargo.toml` — Add `"tray-icon"` to tauri features
- `src-tauri/src/lib.rs` — Build tray icon in `.setup()` with `TrayIconBuilder`. Click → show+focus main window. Add `update_tray_tooltip` Tauri command.
- `src/App.tsx` — `useEffect` on `totalUnread` → invoke `update_tray_tooltip`

---

### Step 10: ROADMAP Update

- `ROADMAP.md` — Mark all Phase 1-8 tasks as `[x]` after verifying each is implemented

---

## Verification

After each step:
```bash
cargo clippy --manifest-path src-tauri/Cargo.toml -- -W clippy::all
cargo test --manifest-path src-tauri/Cargo.toml
npm run build
npx prettier --check src/
```

Manual tests:
- OSC: `echo -e '\033]9;Hello\007'` and `echo -e '\033]777;notify;Title;Body\007'` in unfocused workspace → notification appears
- Ring: notification on unfocused pane → blue glow; focus pane → glow clears
- Sidebar: notification preview text visible under workspace name
- Title: unread count in window titlebar
- read-screen: `forktty read-screen` returns terminal content
- Flash: Alt+Arrow between panes → brief blue flash
- DnD: drag workspace entry in sidebar → reorders
- Tray: tray icon visible, tooltip shows unread count, click opens window
