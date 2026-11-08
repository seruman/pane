#!/bin/sh
# rustfmt this workspace's crates, leaving the Rune checkout (a path dependency) alone.
exec cargo fmt -p pane-ui -p vimkeys -p mdview -p gallery -p mdreview "$@"
