#!/usr/bin/env bash
# The worktrees of the repository in MMENU_VALUE as JSONL, without a bare repository's own entry.
set -o pipefail
here=$(git rev-parse --show-toplevel 2>/dev/null)
git -C "$MMENU_VALUE" worktree list --porcelain -z | jq -Rsc --arg here "$here" '
	reduce (split("\u0000")[] | split(" ")) as $field ({rows: [], record: {}};
		if $field == [] then .rows += [.record] | .record = {}
		else .record[$field[0]] = (if ($field | length) > 1 then $field[1:] | join(" ") else true end) end) |
	.rows[] | select(.worktree and (.bare | not)) |
	{id: .worktree, title: (.branch // "detached" | ltrimstr("refs/heads/")), subtitle: .worktree,
	 detail: (if .worktree == $here then "Current" else "" end), value: .worktree}
'
