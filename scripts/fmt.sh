#!/bin/sh
# rustfmt this workspace's crates, leaving the Rune checkout (a path dependency) alone.
exec cargo fmt -p rex-ui -p rex-vim -p rex-md -p rex-pane "$@"
