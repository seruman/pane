#!/bin/sh
# Repositories as JSONL: the ghq name as the title, the full path as the value.
ghq list -p | while IFS= read -r p; do
  name=${p#"$HOME"/src/}
  printf '{"id":"%s","title":"%s","value":"%s"}\n' "$p" "$name" "$p"
done
