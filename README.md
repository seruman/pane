# pane

Apps and components for programs that draw inside a terminal pane (Rex, Ghostty, kitty) with
[Rune](https://github.com/anomalyco/rune) through its terminal shell, in the terminal's own colours.

| Path | What |
| --- | --- |
| `crates/pane-ui` | Rex-style components: buttons, fields, toggles, sliders, selects, settings groups, lists, menus, dialogs, the command palette, toasts, and layout that wraps and stacks in a narrow pane. See its README. |
| `crates/vimkeys` | Vim's keys over any text, for read-only views: motions, text objects, search and yank as bindings in a `Keymap`, to which an app adds its own actions, operators and a leader. Knows nothing about what draws the text. |
| `apps/mdview` | A Markdown viewer read with Vim's keys: GFM tables, task lists (Space ticks the one under the cursor), code with a copy button, diff and Mermaid fences. |
| `apps/mdreview` | Review a Markdown document the way a pull request is reviewed: select with Vim's keys, comment (`c`) or mark for removal (`d`), with the comments under their blocks; `:wq` writes the review as Markdown feedback. |
| `apps/gallery` | Every pane-ui component on one page. |

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
scripts/check.sh          # checks formatting, build, clippy, tests and docs
RUNE_TERMINAL=1 ./target/release/mdview README.md
RUNE_TERMINAL=1 ./target/release/mdreview PLAN.md -o review.md
RUNE_TERMINAL=1 ./target/release/gallery
```

Without `RUNE_TERMINAL=1` they open a window. `--snap out.png --size 800x600` renders a frame
without one, as any Rune app does.

## Names

The crate and app names are working names. To rename one, `git mv` its folder and replace its
package name and its Rust name (with `_` for `-`) as whole words in the manifests and the code.
`cargo build` then shows anything missed.
