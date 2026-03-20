# Glob: src/**/*.{ts,tsx}

## React Frontend Rules

- TypeScript strict mode, never use `any`
- Functional components only, no class components
- State management via Zustand stores, no prop drilling beyond 2 levels
- xterm.js: Canvas renderer by default (WebGL has bugs on WebKitGTK)
- Terminal resize: ResizeObserver → fitAddon.fit()
- PTY data: Channel.onmessage → term.write(), invoke('pty_write') for input
- Split panes: react-resizable-panels, not Allotment
- Prefer interfaces over types for object shapes
- Run `npx prettier --check src/` before committing
- Lazy-load infrequent panels: `React.lazy(() => import(...))` + `<Suspense fallback={null}>`

## Common Mistakes (Frontend-specific)

- **Don't** fire Zustand writes on every mouse pixel — **Do**: debounce with `requestAnimationFrame`
- **Don't** use `console.log` in production — **Do**: `showToast()` for user feedback, `writeLog()` for structured logs, `logError()` for fire-and-forget
- **Don't** use empty `.catch(() => {})` — **Do**: `.catch(logError)`. Only exception: inside `logError` itself (last-resort sentinel)
- **Don't** set CSP to null in tauri.conf.json — Tauri v2 default CSP is protective

## Debugging (Frontend-specific)

- **Canvas rendering issues**: CanvasAddon must load before any `term.write()`. Check load order.
- **Session restore creates empty workspaces**: Session file stores layout only, not PTY state. Each pane spawns fresh shell.
- **Log files**: `~/.local/share/forktty/logs/forktty-YYYY-MM-DD.log`. Auto-pruned after 30 days.
