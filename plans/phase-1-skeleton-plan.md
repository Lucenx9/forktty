# Phase 1 — Skeleton (MVP Terminal) Plan

## Objective

Bootstrap the ForkTTY Tauri v2 application from zero to a working single-terminal app. The user launches the app, gets a bash shell in an xterm.js terminal, can type commands, see output, and resize the window with proper terminal reflow. Full-screen TUI programs (htop, vim, less) must render correctly. This phase produces the foundational PTY↔frontend data pipeline that all subsequent phases build on.

## Technical Approach

### Architecture

```
[User keystrokes] → xterm.js onData → invoke('pty_write') → portable-pty writer
[Shell output]    → portable-pty reader (spawn_blocking) → Channel<String>.send()
                  → Tauri IPC → Channel.onmessage → xterm.js term.write()
[Window resize]   → ResizeObserver → FitAddon.fit() → invoke('pty_resize')
                  → portable-pty master.resize()
```

### Key Decisions

1. **Tauri v2 + React + Vite**: Scaffold with `cargo tauri init` / `npm create tauri-app`
2. **portable-pty 0.9**: Blocking reader wrapped in `tokio::task::spawn_blocking`. Writer stored as `Arc<Mutex<Box<dyn Write + Send>>>` (one-shot `take_writer()`). Drop slave after spawn.
3. **Tauri Channel<String>**: Push-based, ordered streaming for PTY output. NOT events (too slow), NOT WebSocket (no AttachAddon in Tauri).
4. **Canvas renderer**: Default. Try WebGL on startup with catch-and-fallback due to WebKitGTK bugs.
5. **Base64 encoding for PTY data**: PTY output is raw bytes. Encode as base64 in Rust, decode in frontend before writing to xterm.js. This avoids issues with binary data over JSON IPC.

### Data Flow Detail

**PTY Spawn**: `pty_spawn` Tauri command creates a new PTY via `portable-pty`, spawns the user's shell (`$SHELL` or `/bin/bash`), stores the PTY handle in a `HashMap<u32, PtyHandle>` behind a `Mutex`, starts a background read loop that pushes output to a Tauri Channel, and returns the PTY ID.

**PTY Write**: `pty_write` Tauri command looks up the writer by PTY ID, writes bytes to the PTY master.

**PTY Resize**: `pty_resize` Tauri command looks up the PTY master by ID, calls `resize()` with new cols/rows.

**Cleanup**: When the child process exits, the read loop detects EOF, notifies the frontend, and cleans up the PTY handle.

## Tasks

### Backend (Rust)

- [ ] **1. Scaffold Tauri v2 project**
  - Run `npm create tauri-app@latest` with React + TypeScript template
  - Verify `cargo tauri dev` produces a window with the React dev server
  - Configure `tauri.conf.json`: window title "ForkTTY", default size 1200x800, decorations on

- [ ] **2. Add portable-pty dependency and PTY manager module**
  - Add `portable-pty = "0.9"` and `base64 = "0.22"` to `Cargo.toml`
  - Create `src-tauri/src/pty_manager.rs`:
    - `PtyHandle` struct: holds `Arc<Mutex<Box<dyn Write + Send>>>` (writer), `Arc<Mutex<Box<dyn MasterPty + Send>>>` (master for resize), child handle
    - `PtyManager` struct: `HashMap<u32, PtyHandle>` with next-ID counter, behind `Arc<Mutex<>>`
    - `fn spawn(shell: &str, cols: u16, rows: u16) -> Result<u32>`: create PTY pair, spawn command, drop slave, store handle, return ID
    - `fn write(id: u32, data: &[u8]) -> Result<()>`: look up writer, write bytes
    - `fn resize(id: u32, cols: u16, rows: u16) -> Result<()>`: look up master, call resize
    - `fn kill(id: u32) -> Result<()>`: kill child, remove from map

- [ ] **3. Implement Tauri commands**
  - `pty_spawn(channel: Channel<String>) -> Result<u32, String>`: call PtyManager::spawn, start `spawn_blocking` read loop that reads from PTY reader, base64-encodes output, sends via `channel.send()`. On EOF, send a sentinel message.
  - `pty_write(id: u32, data: String) -> Result<(), String>`: decode input, call PtyManager::write
  - `pty_resize(id: u32, cols: u16, rows: u16) -> Result<(), String>`: call PtyManager::resize
  - Register all commands in `main.rs` via `.invoke_handler(tauri::generate_handler![...])`
  - Manage `PtyManager` as Tauri state via `.manage()`

- [ ] **4. Wire up Tauri app entry point**
  - `main.rs`: create Tauri app, register commands, manage PtyManager state
  - Add `tokio` dependency with `rt-multi-thread` and `macros` features
  - Add `thiserror` for error types
  - Ensure clean compilation with `cargo clippy`

### Frontend (React + TypeScript)

- [ ] **5. Set up frontend dependencies**
  - `npm install @xterm/xterm @xterm/addon-fit @xterm/addon-canvas @xterm/addon-webgl`
  - `npm install -D @types/node`
  - Configure TypeScript strict mode in `tsconfig.json`

- [ ] **6. Create pty-bridge module**
  - `src/lib/pty-bridge.ts`: typed wrappers around `invoke('pty_spawn')`, `invoke('pty_write')`, `invoke('pty_resize')`
  - Handle Channel setup for PTY output streaming
  - Export `spawnPty(onOutput: (data: Uint8Array) => void): Promise<number>`
  - Export `writePty(id: number, data: string): Promise<void>`
  - Export `resizePty(id: number, cols: number, rows: number): Promise<void>`

- [ ] **7. Create TerminalPane component**
  - `src/components/TerminalPane.tsx`:
    - Mount xterm.js Terminal instance in a `useRef<HTMLDivElement>` container
    - Load CanvasAddon (default), try WebglAddon with catch-and-fallback
    - Load FitAddon, call `fit()` on mount and on resize
    - Set up ResizeObserver on container → `fitAddon.fit()` → `resizePty(id, cols, rows)`
    - On mount: call `spawnPty()`, wire `term.onData` → `writePty()`, wire channel output → `term.write()`
    - Cleanup on unmount: dispose terminal, kill PTY

- [ ] **8. Wire App.tsx**
  - `src/App.tsx`: render full-viewport `<TerminalPane />` with no sidebar or chrome yet
  - Minimal CSS: terminal fills entire window, no padding/margin, dark background
  - Import xterm.js CSS

- [ ] **9. Basic window configuration**
  - `tauri.conf.json`: title "ForkTTY", default size 1200x800
  - Dark background color matching terminal (no white flash on startup)
  - Verify min/max/close buttons work correctly on Linux

### Integration & Testing

- [ ] **10. End-to-end verification**
  - `cargo tauri dev` launches app with working terminal
  - Type commands, see output (echo, ls, pwd)
  - Run `htop` — verify full-screen rendering, keyboard input, quit with `q`
  - Run `vim` — verify cursor positioning, insert mode, :q to quit
  - Run `less /etc/passwd` — verify scroll, search, `q` to quit
  - Resize window — terminal reflows text correctly
  - `Ctrl+D` or `exit` — shell exits cleanly (handle EOF gracefully)

- [ ] **11. Code quality checks**
  - `cargo clippy --manifest-path src-tauri/Cargo.toml -- -W clippy::all` passes
  - `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` passes
  - `npm run build` succeeds
  - `npx prettier --check src/` passes

## Acceptance Criteria

Per ROADMAP.md Phase 1:

1. **Launch app → working bash shell**: `cargo tauri dev` opens a window with a functional terminal
2. **Resize window → terminal reflows**: ResizeObserver + FitAddon + pty_resize pipeline works
3. **htop renders correctly**: Full-screen TUI app with colors, dynamic updates, keyboard input
4. **vim renders correctly**: Cursor positioning, mode switching, syntax highlighting
5. **less renders correctly**: Scrolling, search, quit

Additional:
6. **No clippy warnings**: Clean `cargo clippy` output
7. **No TypeScript errors**: Clean `npm run build`
8. **Canvas renderer active**: Verify canvas-based rendering (not DOM fallback)
9. **Clean PTY lifecycle**: Shell exit → no orphan processes, no errors in console

## Files to Create

```
src-tauri/
├── Cargo.toml              # (create) Tauri + portable-pty + tokio + thiserror + base64
├── tauri.conf.json         # (create) Window config, commands, permissions
├── build.rs                # (create) Tauri build script
├── src/
│   ├── main.rs             # (create) Tauri app entry point, register commands + state
│   ├── pty_manager.rs      # (create) PTY lifecycle: spawn, write, resize, kill
│   └── lib.rs              # (create) Module declarations
src/
├── App.tsx                 # (create) Root component, renders TerminalPane
├── App.css                 # (create) Full-viewport terminal styles
├── main.tsx                # (create) React entry point
├── components/
│   └── TerminalPane.tsx    # (create) xterm.js wrapper with PTY bridge
├── lib/
│   └── pty-bridge.ts       # (create) Tauri invoke wrappers for PTY commands
├── vite-env.d.ts           # (create) Vite type declarations
package.json                # (create) Dependencies: react, xterm.js, tauri API
tsconfig.json               # (create) Strict TypeScript config
tsconfig.node.json          # (create) Node TypeScript config for Vite
vite.config.ts              # (create) Vite config with Tauri plugin
index.html                  # (create) HTML entry point
```

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| WebGL crashes on WebKitGTK | Canvas renderer is the default; WebGL is try-catch only |
| portable-pty blocking API causes async issues | Dedicated `spawn_blocking` task for read loop |
| Binary PTY data corrupted over JSON IPC | Base64 encode in Rust, decode in frontend |
| PTY reader thread leaks on shell exit | EOF detection in read loop triggers cleanup |
| `take_writer()` called twice panics | Call once at spawn, store in Arc<Mutex<>> |

## Dependencies

- System: `libwebkit2gtk-4.1-dev`, Rust 1.88+, Node.js 18+
- Rust crates: `tauri` v2, `portable-pty` 0.9, `tokio` (rt-multi-thread), `thiserror`, `base64`
- npm packages: `@tauri-apps/api`, `@tauri-apps/cli`, `@xterm/xterm`, `@xterm/addon-fit`, `@xterm/addon-canvas`, `@xterm/addon-webgl`, `react`, `react-dom`, `typescript`, `vite`, `@vitejs/plugin-react`
