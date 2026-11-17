# mdreview notes

## Now: mdreview itself

- Done (723db25): mouse like nvim (click moves the cursor; drag, double- and triple-click select
  in Visual mode for `c`, `d`, `gy`), `gx` and clicking open links. Not yet tried in real Rex.
- Over SSH, links open on the remote machine.

## Later: pi extension

Scope is plannotator's annotate features only, not its plan mode or code review:

- `/mdreview-last`: review the agent's last message.
- `/mdreview <file>`: review a Markdown file.

Shape from the research:

- Open mdreview in Rex (`rex run --split=right --wait`, or a floating layer as ghqr does) and
  return at once so pi stays usable. Outside Rex there is nowhere to draw.
- When mdreview exits, read its feedback file and send it to the agent as a follow-up message.
- mdreview needs distinct outcomes for this: approved, feedback sent, cancelled (exit status
  or a small JSON result).
- `pi-things/extensions/split-fork.ts` already detects Rex and calls `rex split`.
- Undecided: where the extension lives (Pane or pi-things), and split vs floating layer.
