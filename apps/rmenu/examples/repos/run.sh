#!/bin/sh
here=$(cd "$(dirname "$0")" && pwd) || exit 1
config=$(mktemp -t rmenu-repos.XXXXXX) || exit 1
trap 'rm -f -- "$config"' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
jq --arg here "$here" '
    .sources[].command[0] |= ($here + "/" + ltrimstr("./")) |
    .preview[0] |= ($here + "/" + ltrimstr("./"))
' "$here/config.json" >"$config" || exit 1
"${RMENU_BIN:-$here/../../../../target/release/rmenu}" --config "$config" --json "$@"
