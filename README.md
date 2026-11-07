# rex-kit

Apps and components for programs that draw inside a terminal pane (Rex, Ghostty, kitty) with
[Rune](https://github.com/anomalyco/rune) through its terminal shell, in the terminal's own colours.

| Path | What |
| --- | --- |
| `crates/rex-ui` | Rex-style components: buttons, fields, toggles, sliders, selects, settings groups, lists, menus, dialogs, the command palette, toasts, and layout that wraps and stacks in a narrow pane. See its README. |
| `crates/rex-vim` | Vim's motions, text objects, search and yank over any text, for read-only views. Knows nothing about what draws the text. |
| `apps/rex-md` | A Markdown viewer read with Vim's keys: GFM tables, task lists (Space ticks the one under the cursor), code with a copy button, diff and Mermaid fences. |
| `apps/rex-pane` | Every rex-ui component on one page. |

## Rune

The workspace depends on a Rune fork that adds the terminal shell (`RUNE_TERMINAL=1`), the
terminal's colours (`rune::terminal_colors()`), OSC 52 copy, and a Markdown hook
(`markdown::RenderedText`): the `terminal-shell` branch of github.com/seruman/rune. It points at
a local worktree of that branch in the root `Cargo.toml`; switch to `git` and `rev` once pushed. Everything else
here is plain Rune and rune-kit.

The toolchain is pinned to Rune's (`rust-toolchain.toml`); move them together.

## Run

```sh
cargo build --release
scripts/fmt.sh            # rustfmt; `cargo fmt --all` would also reformat the Rune checkout
RUNE_TERMINAL=1 ./target/release/rex-md README.md
RUNE_TERMINAL=1 ./target/release/rex-pane
```

Without `RUNE_TERMINAL=1` they open a window. `--snap out.png --size 800x600` renders a frame
without one, as any Rune app does.
