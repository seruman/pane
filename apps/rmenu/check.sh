#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$root"
export CARGO_INCREMENTAL=0
export CARGO_BUILD_JOBS=2
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_DEBUG=0
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target/check}"

cargo fmt --package rmenu-core --package rmenu -- --check
cargo build --workspace --jobs 2
cargo test --workspace --jobs 2
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings
python3 -I examples/check-ghqr.py
if [ -f examples/check-repos.py ]; then
    python3 -I examples/check-repos.py
fi
python3 -I tests/pty/check.py --binary "${RMENU_BIN:-$CARGO_TARGET_DIR/debug/rmenu}"
