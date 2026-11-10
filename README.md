# pane

Markdown readers and review tools for [Rune](https://github.com/anomalyco/rune).
The apps draw inside terminals that support the kitty graphics protocol, including
Rex, Superlogical's terminal multiplexer. They use the terminal's colours.

| Package | Purpose |
| --- | --- |
| `pane-ui` | Buttons, forms, lists, menus, dialogs, and overlays for Rune apps. |
| `vimkeys` | Vim motions, selections, search, actions, operators, and a leader key. It does not depend on the renderer. |
| `gallery` | An interactive page of pane-ui components. |
| `mdview` | A Markdown reader with Vim navigation and task checkboxes that write changes to the open file. |
| `mdreview` | A Markdown review app with comments under their document blocks. `:wq` writes the review as Markdown feedback. |

Both Markdown apps draw images from files, with paths relative to the open document. They do
not fetch remote images, which show their alt text instead.

See the [component reference](crates/pane-ui/README.md) and the
[gallery screenshot](docs/gallery.png).

## Build against the local Rune checkout

The workspace uses the terminal-shell changes in [seruman/rune](https://github.com/seruman/rune).
The current dependency configuration requires a local worktree. The `rune` and
`rune-kit` paths in `Cargo.toml` point to
`../../../../tmp/worktrees/github.com/seruman/rune/terminal-shell/crates/`.
A fresh clone without that checkout cannot build. A published dependency remains
pending the repository's remote decision.

`rust-toolchain.toml` selects the Rust channel. `scripts/check.sh` compares that
channel with the local Rune checkout before running Cargo. Update both toolchain
files together. The check scripts require Python 3.11 or newer.

## Build and run the apps

Run these commands from the repository root after setting up the local dependency.

```sh
CARGO_INCREMENTAL=0 scripts/check.sh
CARGO_INCREMENTAL=0 cargo build --release
RUNE_TERMINAL=1 ./target/release/mdview README.md
RUNE_TERMINAL=1 ./target/release/mdreview README.md -o review.md
RUNE_TERMINAL=1 ./target/release/gallery
```

`RUNE_TERMINAL=1` selects Rune's terminal shell. Without that setting, the apps open
native windows. In mdreview, `:wq`, `:x`, and `ZZ` send the review. `:q!` and `ZQ`
quit without sending it. The review app does not edit the document.

## Rename a crate or app

Use `git mv` to rename its folder. Replace its package name in the workspace and
member manifests. Replace the Rust crate name in imports, using underscores where
the package name has hyphens. Run `scripts/check.sh` to find missed references.
