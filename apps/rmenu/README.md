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

## ghqr: the repository picker in a Rex floating layer

`examples/ghqr.fish` is mmenu's `ghqm` on rmenu: repositories, then a repository's worktrees
(Ctrl-W) or memos (Alt-N), in one picker.

```fish
source ~/tmp/rmenu/examples/ghqr.fish
ghqr [query]
```

- Enter changes the calling shell's directory. Inside Rex, Ctrl-V, Ctrl-S and Ctrl-T open the
  choice in a split right, a split below or a project tab, anchored at the calling block.
- Inside Rex (`TERM_PROGRAM=rex` with `REX_SESSION` and `REX_BLOCK` set) the picker opens in a
  floating layer over the calling window and closes when you choose or cancel. Elsewhere it
  draws in the current terminal.
- It needs `ghq`, `jq`, Git with `git-wt` and `git-memo`, and inside Rex the `rex` CLI and the
  `_git_dir_action` Fish function from the dotfiles. It finds rmenu at `RMENU_BIN`, else this
  checkout's `target/release/rmenu`.
- An error from rmenu (exit 2) prints on the calling shell. A layer that never ran the picker,
  or was closed under it, is reported and returns 1 rather than passing for a cancel.
