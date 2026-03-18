# Glob: src-tauri/**/*.rs

## Rust Backend Rules

- Use `thiserror` for error types, never `anyhow` in library code
- All Tauri commands return `Result<T, String>` (Tauri IPC requires serializable errors)
- PTY output streaming uses `Channel<String>`, never events
- portable-pty reader is blocking — always wrap in `tokio::task::spawn_blocking`
- `take_writer()` is one-shot — store in `Arc<Mutex<Box<dyn Write + Send>>>`
- Drop the slave PTY after `spawn_command()` so master gets EOF on child exit
- Run `cargo clippy -- -W clippy::all` before committing
- Run `cargo fmt` before committing
- Prefer `git2` crate over shelling out to git
