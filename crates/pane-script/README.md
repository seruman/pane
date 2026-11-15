# pane-script

Runs a folder of Luau UI with Rune's own `rune-luau` (`crates/rune-luau` in the Rune checkout),
in a native window or Rune's terminal shell. Save a `.luau` file in the folder and the UI
reloads in place, keeping the script's state.

From the workspace root:

```sh
cargo run -p pane-script -- crates/pane-script/examples/counter
RUNE_TERMINAL=1 cargo run -p pane-script -- crates/pane-script/examples/counter
```

The argument is a folder with an `init.luau`; a path to a `.luau` file means its folder.
Terminal mode needs a controlling TTY, a GPU, and Kitty graphics support (e.g. Rex). Rune's
headless flags work too: `--snap out.png`, `--tree out.json`.

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

- Colours are `rune-luau`'s default palette names (`bg`, `text`, `text-secondary`,
  `control-off`, `hover`, `press`, ...), which follow light and dark, or `ui.hex("#rrggbb")`.
- `saveHotState`/`loadHotState` carry state across a saved edit. A save that fails to compile
  or start keeps the running script; the error goes to stderr.
- A render that raises shows an error card with a Reload button. Each entry into the script is
  stopped after 250 ms, and its heap is capped at 64 MB.

See `rune-luau`'s README for the full node and prop list, `task`, signals, `require` and
`ui.virtualList`.
