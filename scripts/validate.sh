#!/usr/bin/env bash
# ZenJooo · Host checks. Rust failures remain failures when cargo is available.
set -euo pipefail
PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_DIR"
node scripts/test-glass-ui.cjs
node scripts/test-webui-dom.cjs
node scripts/test-glass-performance.cjs
python3 scripts/generate-profiles.py --check
node scripts/check-profiles.cjs
python3 scripts/test-profiles.py
python3 scripts/test-installer.py
python3 scripts/test-boot-service.py
python3 scripts/test-webui-updater.py
python3 scripts/test-cloud-updater.py
if command -v cargo >/dev/null 2>&1; then
  TEST_ARGS=()
  if [ ! -r /proc/self/stat ]; then
    echo "NOT RUN: live-process identity test (build host has no /proc)." >&2
    TEST_ARGS=(-- --skip process_identity::regression_tests::live_unrelated_process_is_not_accepted_as_daemon)
  fi
  cargo test --manifest-path native/Cargo.toml --locked --offline "${TEST_ARGS[@]}"
else
  echo "NOT RUN: Rust tests (cargo is missing). Android build requires Rust and NDK." >&2
fi
