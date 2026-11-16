#!/bin/sh
set -e
cd "$MMENU_VALUE"
# Assignment-only substitutions preserve git's exit status. Running them inside echo would hide failures.
branch=$(git branch --show-current)
last=$(git log -1 --format='%h %s (%cr)')
echo "$branch  $last"
echo
git log --oneline -8
echo
for readme in README* readme*; do
	if [ -f "$readme" ]; then
		head -40 "$readme"
		break
	fi
done
