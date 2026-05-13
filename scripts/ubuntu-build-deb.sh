#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

exec "$ROOT_DIR/scripts/ubuntu-dev.sh" bash -lc "npm ci && npm run tauri:build -- --bundles deb"
