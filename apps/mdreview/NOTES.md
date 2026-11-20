# mdreview notes

## Done

- Mouse like nvim (click moves the cursor; drag, double- and triple-click select in Visual
  mode for `c`, `d`, `gy`), `gx` and clicking open links.
- `\w` and `:w` write the review and keep reviewing; `\q` quits once nothing is unwritten.
- The title bar is the window's titlebar in the GUI, clear of the macOS traffic lights.
- GFM extras, `<details>` folds and footnote jumps (see below). Alerts are tinted.
- Tab lists the comments from Visual mode too; comment buttons aren't Tab stops.
- The comment box floats next to the selection instead of under the block, which put it off
  screen in long code blocks and lists. No quote, no heading, accent caret, smaller corners.
  Enter adds a line, Ctrl-Enter saves, Esc or a click outside cancels.

## Open

- Over SSH, links open on the remote machine.
- Save and Cancel clicks were reported as doing nothing in Rex. They work headless and in a
  fake kitty terminal at scale 1 and 2. Retry with a fresh binary; if it persists, capture
  `RUNE_TERMINAL_DEBUG=/tmp/mdr.log` to see whether Rex delivers the mouse events.
- Terminal colours can differ between Rex clients. Colour queries (OSC 10/11/12/4) are answered
  by the Rex *clients*, not the server, so with two clients attached (the home Mac's app and a
  remote one) the answer may depend on which client replies, and Rune keeps the last reply.
  Seen: a fish probe in a pane got text `#6b5c4d` on `#f4f0ed`, while an mdreview screenshot had
  `#3f4651` on `#faf6ee` (a blue-grey text that tints rune-kit's layers, so table headers came
  out light blue `#e8f0fe`; with `#6b5c4d` on `#f4f0ed` they are a warm `#f9e7d5`). To confirm,
  run with `RUNE_TERMINAL_DEBUG` and look for two sets of colour replies. If so, it's a question
  for Rex: which client should answer for a pane.
- Table headers use rune-kit's `layer_01`, which takes its hue from the more saturated of the
  terminal's text and background. Everything else pane-ui draws uses `theme.group`. Making the
  header use `theme.group` would keep tables neutral on any theme.

## Markdown coverage against GFM

Rune's Markdown (our fork, `rune-kit/src/markdown`) now draws footnotes, alerts, inline HTML
(`kbd`, `sub`, `sup`, `mark`, `ins`, `u`, `s`, `b`, `i`, `code`, `br`, `a href`), `<details>`,
`:shortcode:` emoji, and other HTML blocks as their source. mdreview splits the same blocks.

- `<details>` fold: `za` `zo` `zc` `zR` `zM`, or click the triangle. `j`/`k` skip a closed one;
  a search or jump into one opens it.
- Footnotes: `gx` on a reference goes to the note, `gx` in the note goes back.
- Not cached: since the upstream merge (Rune `8b66b203`), Rune caches settled Markdown blocks
  across rebuilds (`markdown/memo.rs`). Alerts, `<details>` and footnotes opt out (`HAS_GFM`;
  `hash_block_direct` returns false), because their renderer (`render/gfm.rs`) draws nested
  blocks through a fresh `blocks_with_dirs` call with no `PreBlock`s and its own `next_details`
  counter. To cache them: digest their children like quotes (`rebuild_quote_meta`), hash
  `details_open` and the `next_details` position, and pass the inner `PreBlock`s down.
- Not done: math is shown as code, not typeset (not important). Sub/superscript are smaller text
  without a baseline shift, which Rune text can't do. Bare emails aren't linked (on purpose).

## Later: pi extension

Lives in pi-things on the `mdreview` branch: `/mdreview <file>` and `/mdreview-last` open
mdreview in a Rex floating layer and send what `:wq` writes as the next message. Possible next:
pick an earlier reply, an approve outcome, an agent tool.
