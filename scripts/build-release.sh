#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
python3 scripts/check-capabilities.py
scripts/mem-guard.sh 1600 cargo build --release --locked -j1
scripts/mem-guard.sh 1600 env CARGO_PROFILE_RELEASE_OPT_LEVEL=2 CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 cargo build --manifest-path ../qaqh-backend/Cargo.toml --target-dir "$PWD/target" -p qaqh-daemon --release --locked -j1
python3 scripts/package.py
