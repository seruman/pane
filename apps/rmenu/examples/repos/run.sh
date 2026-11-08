#!/bin/sh
# Your ghq repositories in rmenu, with worktrees (Ctrl-W), a preview (Alt-P) and marks (Tab).
# Prints the --json result; set RMENU_BIN to use another build.
cd "$(dirname "$0")" || exit 1
exec "${RMENU_BIN:-../../target/release/rmenu}" --config config.json --json "$@"
