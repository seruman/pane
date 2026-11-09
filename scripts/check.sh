#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
python3 scripts/check_toolchain.py
python3 scripts/generate_icons.py --check
python3 -B -I -m unittest discover -s scripts -p 'test_*.py'
cargo fmt --check
cargo build --workspace
cargo clippy --workspace --all-targets --no-deps -- -D warnings
cargo test --workspace --quiet
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --quiet
echo "check: ok"
