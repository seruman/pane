# pane-script

A small Luau UI host. One file, one sandbox, one window; save the file to rebuild
its UI without restarting. No changes to Rune or the existing Pane apps.

From the workspace root:

```sh
cargo run -p pane-script -- crates/pane-script/examples/counter.luau
RUNE_TERMINAL=1 cargo run -p pane-script -- crates/pane-script/examples/counter.luau
```

The terminal mode needs a controlling TTY, a GPU, and Kitty graphics support
(e.g. Rex). Click the buttons, or use Tab and Space. Rune's headless tools also
work, for example:

```sh
cargo run -p pane-script -- crates/pane-script/examples/counter.luau --tree /tmp/counter.json
cargo test -p pane-script
```

## Script contract

Return a table containing three functions:

```luau
return {
    init = function(previous)
        return previous or { count = 0 }
    end,
    update = function(state, message)
        state.count += 1
        return state
    end,
    view = function(state)
        return ui.column({
            ui.text("Count: " .. state.count),
            ui.button("Increment", "increment"),
        })
    end,
}
```

- `init(previous)` receives `nil` on the first load, then the last committed state
  on reload. It can migrate the previous state to a new shape.
- `update(state, message)` returns the next state. Buttons send string messages.
- `view(state)` returns one UI node. Mutating state inside `view` has no effect on
  the committed state; keep state changes in `update`/`init`.
- `ui.text(string)`, `ui.button(label, message)`, `ui.row({nodes})`, and
  `ui.column({nodes})` are the entire UI API. Layout and colors use Pane's theme;
  buttons use `pane-ui::Button`. No arbitrary style properties yet.

State is bounded plain data: nil, booleans, finite numbers, UTF-8 strings, and
plain tables with string/integer keys. Functions, metatables, cycles, userdata,
and threads are not state. Shared table identity is not preserved. Store all
persistent state here, not in globals or closures: reload creates a new VM.

## Reload and failure behavior

The parent directory is watched with `notify`, including atomic rename saves.
Notifications wake Rune through its setup `Waker`; an 80 ms debounce coalesces
editor writes. Unchanged successful source is not evaluated again.

Loading, initialization, update, and view validation are transactional with
respect to the state snapshot and displayed tree. On any error, an error banner
appears above the last good tree (if any), and its controls pause. Saving a valid
file clears the error and passes the last good state into `init`. A missing file
also shows an error; recreating it recovers. The parent directory must exist for
watching to start. State is in memory only, not persisted across app restarts.

## Limits and scope

The VM uses safe standard libraries, frozen sandbox globals, text-only source,
a memory cap, and a fresh interrupt/deadline budget for each script call. Source
is capped at 1 MiB. State and UI conversion have separate depth, node and string
limits, and use raw table access (no script metamethods during validation).

This is an in-process scripting sandbox, not OS process isolation: native
library work and compilation are not preempted by Luau's interrupt callback.
Do not treat it as a hard real-time scheduler or a boundary for hostile tenants.
There is no filesystem, network, process execution, module loading, or provider
API exposed to scripts. Future providers can deliver messages through the host's
update boundary; they are deliberately not part of this first slice.

Tests run Rune's `Runtime::new`/`frame` headlessly and cover rendering/input,
stateful reload, visible errors and recovery, loop budgets, and real filesystem
watcher wakes including atomic saves and delete/recreate.
