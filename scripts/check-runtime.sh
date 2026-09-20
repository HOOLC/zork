#!/usr/bin/env bash
# Shared by the main workflow and the local pre-push hook.
set -euo pipefail
cd "$(dirname "$0")/.."
eval "$(python3 scripts/lib/build_env.py --shell)"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-0}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
export ZORK_TEST_BIN_DIR="${ZORK_TEST_BIN_DIR:-$CARGO_TARGET_DIR/debug}"
packages=(--workspace --exclude zork-gui --exclude zork-browser-runtime --exclude zork-ui
          --features zork-client-core/desktop)

build() {
  # One target selection unifies development features for programs and tests.
  cargo build --locked "${packages[@]}" --lib --bins --tests
}

tests() {
  python3 scripts/test-build-env.py
  python3 scripts/test-ci-source-stamps.py
  cargo test --locked "${packages[@]}" --lib --tests --no-fail-fast
  pnpm test
  python3 scripts/test-slack-forwarding.py
  python3 scripts/test-skills.py
  python3 scripts/test-sync-idle.py
  python3 crates/zork-gui/tests/test_station_entry.py
}

shared_files() {
  ZORK_TEST_STATION_BIN="$ZORK_TEST_BIN_DIR/zork-station" \
    cargo test --locked -p zork-client-core --features desktop \
      --test shared_files_mesh -- --ignored
}

case "${1:-all}" in
  build) build ;;
  test) tests ;;
  all) build; tests ;;
  shared-files) build; shared_files ;;
  *) echo 'Usage: check-runtime.sh [build|test|all|shared-files]' >&2; exit 2 ;;
esac
