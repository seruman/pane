#!/bin/sh
set -e
cd "$MMENU_VALUE"
# Assignments, so set -e sees a failing git. A substitution inside echo would hide it.
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
