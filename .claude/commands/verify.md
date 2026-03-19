---
description: Run all verification gates (clippy, fmt, test, build, prettier)
allowed-tools: ["Bash", "Read"]
---

Run all ForkTTY verification gates and report results:

1. `cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings`
2. `cargo fmt --manifest-path src-tauri/Cargo.toml --check`
3. `cargo test --manifest-path src-tauri/Cargo.toml`
4. `npm run build`
5. `npx prettier --check src/`

Report each as PASS or FAIL. If any fail, show the error output.
Do NOT fix anything — just report.
