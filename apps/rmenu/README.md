# rmenu

rmenu is a terminal picker that implements mmenu's command-line interface with Rune.
It reads candidates from stdin or configured commands and prints the selected values.

## Build requirements

The current build requires macOS and a terminal with kitty graphics support.
`Cargo.toml` points to a local Rune checkout. That checkout must exist at the configured path before you build.
The dependency uses the checkout's current files, including uncommitted changes.

```sh
CARGO_INCREMENTAL=0 cargo build --release
ghq list -p | target/release/rmenu --prompt Repositories
```

## Input, output, and configuration

`rmenu --help` lists the command-line flags. Input can contain lines, NUL-delimited values, or JSONL records.
The default output contains selected values separated by newlines. `--print0` uses NUL separators instead.
`--json` returns the action context, including selected items and the path through pushed levels.

`--config` reads JSON configuration for sources, bindings, pushed levels, and previews.
The [repository example configuration](examples/repos/config.json) defines two sources and bindings for worktrees, preview, and copy.
The [ghqr example](examples/ghqr.fish) also configures a memo source.

| Exit status | Meaning |
|---|---|
| 0 | Selection completed, or help or version information was printed. |
| 130 | The picker was cancelled. |
| 2 | Arguments, configuration, input, startup, or output failed. |

clap reports invalid command-line arguments. rmenu reports configuration errors.
Both exit with status 2.

## How rmenu differs from mmenu

- Matching uses nucleo's query syntax, including `!x`, `^x`, `x$`, and `'x`, with smart case.
- `--appearance system` uses the terminal's foreground and background colours when available.
  `light` and `dark` use Rune's theme colours.
- `--theme` and the configuration's `theme` field accept the compatibility values `rex` and `native` but do not change the colours.
- rmenu accepts `cmd+<key>` bindings but warns that it cannot dispatch them. Use Ctrl chords instead.
- Copy writes an OSC 52 clipboard request. The terminal must support and allow clipboard writes.
- Refused actions briefly outline the query field instead of ringing the terminal bell.

## Examples

The [repository picker](examples/repos/README.md) lists ghq repositories and prints the selection as JSON.
Ctrl-W opens the worktree level, Alt-P toggles the git preview, Tab marks rows, and Ctrl-Y copies the selection.
The launcher needs `ghq`, `git`, and `jq` on `PATH`.

The fish function `ghqr` opens repository, worktree, and memo levels.
It needs `fish`, `ghq`, `git`, `git-memo`, and `jq`, but does not need an mmenu checkout.
From the rmenu checkout, load and run it with:

```fish
source examples/ghqr.fish
ghqr
```

An optional argument sets the initial query. Outside Rex, ghqr draws in the current terminal and changes to the selected directory.
Inside Rex, it opens a floating layer in the caller's window.
Rex mode also requires the `rex` command and your fish function `_git_dir_action` for split and tab actions.
That function receives the direction, the selected directory, and an optional source-repository label.
Ctrl-V splits right, Ctrl-S splits below, and Ctrl-T opens a project tab.
Ctrl-W opens worktrees, and Alt-N opens memos.

Both examples use `target/release/rmenu` by default. Set `RMENU_BIN` to use another executable.

## Where the code lives

[`crates/core/src/lib.rs`](crates/core/src/lib.rs) declares the core modules.
The model takes events and returns effects without drawing a UI.
[`crates/rmenu/src`](crates/rmenu/src) contains the CLI, the host that runs effects, and the view.

## Checks

`./check.sh` runs formatting, build, tests, Clippy, the example checks, and private-PTY integration tests.
The [PTY test guide](tests/pty/README.md) describes the terminal checks and cleanup rules.
