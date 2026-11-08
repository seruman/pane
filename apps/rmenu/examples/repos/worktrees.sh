#!/bin/sh
git -C "$MMENU_VALUE" worktree list --porcelain | awk -v OFS='' '
  /^worktree /{p=substr($0,10)} /^branch /{b=substr($0,8); sub("refs/heads/","",b)}
  /^detached/{b="detached"} /^$/{ if (p!="") printf "{\"id\":\"%s\",\"title\":\"%s\",\"subtitle\":\"%s\",\"value\":\"%s\"}\n", p, b, p, p; p=""; b="" }
  END{ if (p!="") printf "{\"id\":\"%s\",\"title\":\"%s\",\"subtitle\":\"%s\",\"value\":\"%s\"}\n", p, b, p, p }'
