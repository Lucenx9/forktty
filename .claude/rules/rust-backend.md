# Glob: src-tauri/**/*.rs

## Rust Backend Rules

- Use `thiserror` for error types, never `anyhow` in library code
- All Tauri commands return `Result<T, String>` (Tauri IPC requires serializable errors)
- PTY output streaming uses `Channel<PtyEvent>`, never events
- portable-pty reader is blocking — wrap in `tauri::async_runtime::spawn_blocking`
- `take_writer()` is one-shot — store in `Arc<Mutex<Box<dyn Write + Send>>>`
- Drop the slave PTY after `spawn_command()` so master gets EOF on child exit
- Run `cargo fmt --check` and `cargo clippy -- -D warnings` before committing
- Prefer `git2` crate over shelling out to git

## Common Mistakes (Rust-specific)

- **Don't** use `sh -c` for external commands — **Why**: command injection. **Do**: argv splitting via `split_whitespace()` + `Command::new(prog).args(args)`
- **Don't** pass frontend paths to backend without `canonicalize` — **Why**: path traversal. **Do**: call `verify_repo_path()` which canonicalizes + checks git-workdir boundary
- **Don't** forget `index.write()` before `index.write_tree()` in git2 — **Why**: merge silently fails
- **Don't** allocate in hot paths — output_scanner runs on every PTY read chunk. Avoid `data.to_vec()` when a reference suffices
- **Don't** use `filter_map(ok())` on compile-time constant regex — **Why**: silently drops bad patterns. **Do**: `unwrap_or_else(panic!)`
- **Don't** use `BufReader::lines()` for untrusted input — **Why**: OOM via large line. **Do**: `fill_buf`/`consume` with size checking
- **Don't** use magic numbers for errno — **Do**: use `libc::EIO` instead of bare `5`
- **Don't** use `unwrap_or` for `parent()` on paths — **Do**: return explicit `Result` error

## Debugging (Rust-specific)

- **PTY not receiving input**: Check `take_writer()` called exactly once
- **Socket connection refused**: Check `$XDG_RUNTIME_DIR/forktty.sock` permissions are `srw-------`
- **OSC 133 not detected**: Shell needs prompt integration (`bash-preexec` or zsh hooks). Test: `echo -e '\033]133;A\007'`
- **Worktree operations fail**: Run `git worktree list`, prune stale with `git worktree prune`
- **CI fails "frontendDist path doesn't exist"**: `tauri::generate_context!()` needs `../dist`. Build frontend before cargo clippy.
