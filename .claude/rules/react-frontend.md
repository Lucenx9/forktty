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
