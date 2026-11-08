#!/usr/bin/env bash
# Runs config.json's source commands against a stub ghq and throwaway repositories with awkward paths.
set -u
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failed=0

check() {
	if [ "$2" = "$3" ]; then
		printf 'ok    %s\n' "$1"
	else
		printf 'FAIL  %s\n  want %s\n  got  %s\n' "$1" "$3" "$2"
		failed=1
	fi
}

git() { command git -c init.defaultBranch=main -c user.name=check -c user.email=check@example.com "$@"; }

source_command() {
	local id=$1
	shift
	local command=()
	while IFS= read -r -d '' arg; do command+=("$arg"); done \
		< <(jq -j --arg id "$id" '.sources[] | select(.id == $id) | .command[] | ., "\u0000"' "$here/config.json")
	(cd "$here" && env "$@" "${command[@]}")
}

odd="$work/src/github.com/a/q\"uo\\te"
mkdir -p "$work/bin"
printf '%s\n' "$work/src" "$work/other" >"$work/roots"
printf '%s\n' "$odd" "$work/other/x" "$work/elsewhere/y" >"$work/list"
cat >"$work/bin/ghq" <<'EOF'
#!/bin/sh
case "$1 $2" in
"root --all") cat "$CHECK_WORK/roots" ;;
"list -p") cat "$CHECK_WORK/list" ;;
*) exit 2 ;;
esac
EOF
chmod +x "$work/bin/ghq"

rows=$(source_command repos CHECK_WORK="$work" PATH="$work/bin:$PATH")
check "repos exits 0" "$?" 0
check "repos titles are root-relative ghq names" \
	"$(printf '%s\n' "$rows" | jq -r .title 2>&1)" \
	"$(printf '%s\n' 'github.com/a/q"uo\te' x "$work/elsewhere/y")"
check "repos values are the full paths" \
	"$(printf '%s\n' "$rows" | jq -r .value 2>&1)" \
	"$(cat "$work/list")"

main="$work/w/ma\"in"
git init -q "$main"
git -C "$main" commit -q --allow-empty -m one
git -C "$main" worktree add -q -b 'fe"at' "$work/w/wt\\back"
git -C "$main" worktree add -q --detach "$work/w/detached"
rows=$(source_command worktrees MMENU_VALUE="$main")
check "worktrees exits 0" "$?" 0
check "worktrees titles are branches" \
	"$(printf '%s\n' "$rows" | jq -r .title 2>&1)" \
	"$(printf '%s\n' main detached 'fe"at')"
check "worktrees values are the worktree paths" \
	"$(printf '%s\n' "$rows" | jq -r .value 2>&1 | sed 's|^/private||')" \
	"$(printf '%s\n' "$main" "$work/w/detached" "$work/w/wt\\back" | sed 's|^/private||')"

git init -q --bare "$work/b.git"
git -C "$work/b.git" worktree add -q "$work/bw" 2>/dev/null
rows=$(source_command worktrees MMENU_VALUE="$work/b.git")
check "worktrees skips the bare repository" \
	"$(printf '%s\n' "$rows" | jq -r .value 2>&1 | sed 's|^/private||')" \
	"$(printf '%s\n' "$work/bw" | sed 's|^/private||')"

exit $failed
