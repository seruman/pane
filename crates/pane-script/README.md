# pane-script

Runs a folder of Luau UI with Rune's own `rune-luau` (`crates/rune-luau` in the Rune checkout),
in a native window or Rune's terminal shell. Save a `.luau` file in the folder and the UI
reloads in place, keeping the script's state.

From the workspace root:

```sh
cargo run -p pane-script -- crates/pane-script/examples/counter
RUNE_TERMINAL=1 cargo run -p pane-script -- crates/pane-script/examples/counter
```

`examples/counter` is now the live task-board demo: Now / Next / Done, a task-template
picker, and a shipped count. It migrates a running counter's state; subsequent edits retain
tasks, selection, and the open picker. Free-text entry is not exposed by this Luau host.

The argument is a folder with an `init.luau`; a path to a `.luau` file means its folder.
Terminal mode needs a controlling TTY, a GPU, and Kitty graphics support (e.g. Rex). Rune's
headless flags work too: `--snap out.png`, `--tree out.json`.

## Live agent inspection

Add `--inspect /tmp/pane-agent` to expose an opt-in, private local socket. Agents can query
live trees, keyed geometry and logs, focus controls, dispatch keyboard activation/text, and
scroll. In terminal mode, `screenshot` encodes Rune's retained offscreen frame as PNG—no OS
screen capture, Rex integration, second VM, or duplicate render. Both hosts expose full
accessibility/element trees; the lightweight `elements` operation remains partial geometry.
Hot reload keeps the connection endpoint.

```sh
python3 scripts/pane-inspect.py /tmp/pane-agent '{"op":"elements"}'
python3 scripts/pane-inspect.py /tmp/pane-agent '{"op":"activate","key":"select-1"}'
python3 scripts/pane-inspect.py /tmp/pane-agent '{"op":"screenshot"}' --output /tmp/pane-frame.png
```

PNG output is terminal-only for now. The client creates a new file, never overwriting one.

See [INSPECTION.md](INSPECTION.md) for launch examples, protocol, limitations, and trust model.

## Native I/O

HTTP, process, and filesystem access are opt-in for **trusted scripts**.
`--allow-fs` grants **unsandboxed filesystem access** (including absolute paths and `..`).
`--allow-process` runs commands **as your user, with your full user permissions**.
These flags grant capabilities, not a security sandbox; only enable them for code you trust:

```sh
cargo run -p pane-script -- crates/pane-script/examples/io \
  --allow-http --allow-process --allow-fs
```

`@pane/http`, `@pane/process`, and `@pane/fs` suspend the calling task without blocking the UI.
HTTP bodies, files, and **separate stdout/stderr** use the same Reader interface; `readAll()` and
`proc:collect()` collect streams when wanted. `@pane/json` is always available. The example has
click-to-run HTTP, `gh`, filesystem, and streaming-output demos.

See [HOST_API.md](HOST_API.md) for the complete API, errors, limits, cancellation, and trust model.

## Scripts

This is a thin runner; the script contract, UI nodes, limits and reload behaviour are all
`rune-luau`'s. In short, `init.luau` returns a module whose `render(view)` builds UI with
`require("@rune/ui")`:

```luau
local ui = require("@rune/ui")
local state = { count = 0 }
return {
    render = function(view)
        return ui.col({ pad = 16, gap = 8 }, {
            ui.text(`Count: {state.count}`),
            ui.row({ px = 10, py = 4, radius = 6, fill = "control-off", focusable = true,
                onPress = function() state.count += 1 end }, { "Increment" }),
        })
    end,
    saveHotState = function() return state end,
    loadHotState = function(saved) state = saved end,
}
```

- Colours use Pane's terminal-derived theme: `bg`, `card`, `card-border`, `text`,
  `text-secondary`, `separator`, `control-off`, `control-track`, `indicator`, `hover`,
  and `press`. `accent` follows the terminal cursor colour; `accent-hover`, `accent-text`,
  and `on-accent` provide matching hover, text, and contrasting label colours. Without
  terminal colours, the theme falls back to Merino light/dark. `ui.hex("#rrggbb")` still
  supports explicit colours. Late terminal colour replies invalidate cached script views.
- `saveHotState`/`loadHotState` carry state across a saved edit. A save that fails to compile
  or start keeps the running script; the error goes to stderr.
- A render that raises shows an error card with a Reload button. Each entry into the script is
  stopped after 250 ms, and its heap is capped at 64 MB.

See `rune-luau`'s README for the full node and prop list, `task`, signals, `require` and
`ui.virtualList`.
