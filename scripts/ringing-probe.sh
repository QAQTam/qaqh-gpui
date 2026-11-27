#!/usr/bin/env bash
# RingingV3 无 GUI 连通性探测（本地 IPC + 握手 + session.list）。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
exec cargo run -j2 -p qaqh-gpui -- --probe "$@"
