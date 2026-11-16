#!/bin/sh
set -eu

cd "$(dirname "$0")/../.."
cargo build --package rmenu
bash apps/rmenu/examples/repos/check.sh
python3 -I apps/rmenu/tests/pty/check.py --binary "${RMENU_BIN:-${CARGO_TARGET_DIR:-target}/debug/rmenu}"
