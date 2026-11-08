#!/bin/sh
cd "$MMENU_VALUE" 2>/dev/null || exit 0
echo "$(git branch --show-current 2>/dev/null)  $(git log -1 --format='%h %s (%cr)' 2>/dev/null)"
echo
git log --oneline -8 2>/dev/null
echo
r=$(ls README* readme* 2>/dev/null | head -1); [ -n "$r" ] && head -40 "$r"
