# rmenu

mmenu's command line, drawn by [Rune](../worktrees/github.com/seruman/rune/rmenu-base) inside the
terminal (kitty graphics), so pickers work wherever a pty reaches: a remote host, a Rex floating
layer, SSH.

```sh
cargo build --release
ghq list -p | target/release/rmenu --prompt Repositories
```

Flags, `--config` JSON (sources,
bindings, push levels, preview), stdin formats, `--json` / `--print0` output and exit codes
(0 chosen, 130 cancelled, 2 failed input) follow mmenu's README.

Differences:

- Matching is nucleo's, with its query syntax (`!x`, `^x`, `x$`, `'x`) and smart case.
- Colours come from the terminal, so `--theme` and `--appearance` are accepted and ignored.
- A terminal never sends Command, so `cmd+…` bindings parse but can't fire. Ctrl chords work.
- Usage and config errors are clap's, exit 2.

Layout: `crates/core` holds the model (a reducer: events in, effects out), config, records,
commands and search; `crates/rmenu` holds the CLI, the host that runs effects on threads, and
the view.
