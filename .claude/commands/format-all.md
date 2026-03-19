---
description: Format all Rust and TypeScript code
allowed-tools: ["Bash"]
---

Format the entire codebase:

1. `cargo fmt --manifest-path src-tauri/Cargo.toml`
2. `npx prettier --write src/`

Report what changed.
