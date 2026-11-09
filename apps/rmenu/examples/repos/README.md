# Pick a repository

Run `examples/repos/run.sh` from the rmenu checkout, or use its absolute path from another directory.
Set `RMENU_BIN` to use a different rmenu executable.
A relative `RMENU_BIN` path starts at your current directory.

The launcher requires `ghq`, `git`, and `jq` on `PATH`.
It prints the selected result as JSON.
Use Ctrl-W to list worktrees, Alt-P to toggle the preview, Tab to mark rows, and Ctrl-Y to copy the selection.

`config.json` is a template whose command paths start at this directory.
The launcher writes a temporary configuration with absolute command paths and removes it when rmenu exits.
It keeps your current directory.
To use `config.json` directly with `rmenu --config`, first change to `examples/repos`.
