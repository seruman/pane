#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo fmt --check
cargo build --workspace
cargo clippy --workspace --all-targets --no-deps -- -D warnings
cargo test --workspace --quiet
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --quiet
echo "check: ok"
