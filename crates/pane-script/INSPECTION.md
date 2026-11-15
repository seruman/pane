# Inspect and drive a live Pane script

The bridge operates on the **same Script and Runtime as the running app**, without a second
VM or a headless replica. Offscreen screenshots use Rune's `App::terminal_capture` API.
It is off unless started with `--inspect DIR`. Requires Unix (macOS/Linux). The existing
task-board (`examples/counter`) and I/O examples need no instrumentation.

```sh
# Native window:
cargo run -p pane-script -- crates/pane-script/examples/counter --inspect /tmp/pane-agent

# Or a terminal with Kitty graphics support:
RUNE_TERMINAL=1 cargo run -p pane-script -- crates/pane-script/examples/counter \
  --inspect /tmp/pane-agent

# In another shell, from the workspace root:
python3 scripts/pane-inspect.py /tmp/pane-agent '{"op":"info"}'
python3 scripts/pane-inspect.py /tmp/pane-agent '{"op":"elements"}'
python3 scripts/pane-inspect.py /tmp/pane-agent '{"op":"activate","key":"select-1"}'
```

Use a different directory for each process. Starting an inspected app requires a new launch;
this does not attach to an older binary lacking the bridge. Luau hot reloads keep the endpoint
and running state intact. The bridge does not modify scripts or add script capabilities.

## What can be inspected

- **Both hosts:** `elements` is the latest live runtime frame's **partial keyed geometry**:
  recorded names, rectangles in logical points, effective opacity, and viewport dimensions.
  It uses Rune's existing `Rewind` recorder, configured to retain only the latest frame.
  Names are trace names when supplied, otherwise element keys. They are not paths or stable
  numeric IDs. Unkeyed elements are absent; no text, roles, hierarchy, focusability, or clipping
  metadata is available here. Geometry is quantized to 0.01 points and opacity to 0.001.
- **Native windows:** `tree` requests a drawn frame and returns Rune's full element inventory
  (keys, paths, parent indexes, text, roles, focus, bounds) and accessibility JSON.
- **Terminal:** `tree` returns the full element/accessibility inventory through Rune's
  on-demand terminal capture hook. It includes capture metadata and `matchesCapturedPixels`;
  when `stale` is true, the current runtime tree may be newer than the retained pixel image.
  `elements` remains a lightweight partial inventory for compatibility, not a semantic tree.
- `pick` returns the live runtime's hit identity (path and optional key) at a logical point.
- `logs` returns up to 256 recent script log entries, newest first, and the reload count.

Recorded geometry comes from runtime frame construction, **not confirmation that the terminal
presented those pixels**. Idle apps return their latest recorded frame; no timestamp/freshness
barrier is promised. A native tree may time out while its window cannot draw.

## Offscreen PNG screenshots

In terminal mode, `screenshot` returns Rune's **retained live offscreen image**. An idle capture
reads the existing image; it does not start another VM, rerun the script's render function,
create a second runtime, or take an OS/Rex screenshot. When there is no valid retained image
(e.g. immediately after resize), Rune may attempt one full frame or report a capture error.

```sh
python3 scripts/pane-inspect.py /tmp/pane-agent '{"op":"screenshot"}' --output /tmp/pane-frame.png
python3 scripts/pane-inspect.py /tmp/pane-agent '{"op":"tree"}'
```

The client validates the base64 PNG, creates a **new** file (never overwrites), and prints only
metadata instead of the image data. Without `--output`, the response contains
`image: {format: "png", encoding: "base64", data: "...", width, height, bytes}`.

`capture` contains `frame`, `clock`, `width`, `height`, `scale`, and `stale`. Dimensions are
physical pixels; UI coordinates are logical points. `stale: true` means a newer runtime frame
is due/pending or animating, so the retained image can lag current state. Frame numbers identify
images, not requests. Pixels are the source image Rune supplies to the terminal, **not proof
of terminal acknowledgement or display**. Keep metadata when assessing animation or pairing a
tree with an image.

Borrowed RGBA rows are copied on the UI thread; padding is removed and PNG is encoded on the
inspector worker. Capture requests have unique IDs: timeout cancels the pending request, and
a late result cannot answer a later request. GPU mapping failure/timeout and shutdown are
reported as errors, not empty images. Current limits: 64 MiB packed RGBA and 2 MiB encoded PNG.

Native-window `screenshot` is currently unsupported (its full tree still works). Rune's
`--snap`/`--tree` flags still run a separate headless instance and cannot be combined with
`--inspect`.

## Requests

One UTF-8 JSON line per connection to `DIR/control.sock`, one response, then close. No external
packages are needed by the Python client. It exits nonzero on transport or protocol errors.

| Request | Meaning |
| --- | --- |
| `{"op":"info"}` | Protocol version, PID, host, common operations, tree/screenshot support |
| `{"op":"elements"}` | Partial live keyed geometry, as described above |
| `{"op":"tree"}` | Full live element/accessibility tree; terminal replies include capture metadata |
| `{"op":"screenshot"}` | Base64 PNG from the terminal host’s retained offscreen image |
| `{"op":"pick","x":100,"y":40}` | Hit-test existing runtime geometry |
| `{"op":"logs"}` | Recent script logs and reload count |
| `{"op":"focus","key":"name"}` | Request focus and wait until that key has focus |
| `{"op":"activate","key":"name"}` | Focus, verify the focused key, then dispatch Space |
| `{"op":"text","key":"input","text":"Hello"}` | Focus, verify, then dispatch a logical text key event |
| `{"op":"key","key":"Tab","mods":["shift"]}` | Dispatch to current focus, without targeting a control |
| `{"op":"scroll","key":"list","x":0,"y":300}` | Request an instant absolute scroll offset in logical points |

Use **unique actual element keys**, without conflicting trace names. Missing, transparent,
or duplicate recorded names are rejected for targeted operations. This is a preliminary check,
not a stable identity guarantee across animation/reload. Focus is checked again on the UI thread;
an absent, disabled, or non-focusable target never causes a fallback Space to the previous focus.

`activate` and `text` are **keyboard dispatch**, not semantic role-checked actions or mouse
clicks. Use `activate` only on known buttons: on an input it types a space. Conversely, text
`" "` can activate a focused button. No arbitrary pointer injection is exposed. Real user input
may interleave with automation; don't drive the same controls concurrently. `scroll` confirms
dispatch, not that the target was scrollable. Script callbacks and asynchronous I/O may finish
later; read the UI/logs to confirm their outcome.

Named keys: `Tab`, `Enter`, `Escape`, `Backspace`, `Delete`, `Left`, `Right`, `Up`, `Down`, `Home`,
`End`, `PageUp`, `PageDown`, `Space`, or one non-control character. Modifiers: `shift`, `cmd`,
`ctrl`, `alt`, `logo`. `cmd` means Rune's primary shortcut modifier; modifiers are explicit,
not inferred from the operating system. Text is a logical key event, not clipboard paste/IME.

Success: `{"ok":true,"result":...}`. Error: `{"ok":false,"error":"..."}`.
Unknown operations/fields and invalid types are rejected. A `dispatched` result is not proof
of a handler's eventual outcome. Timed-out jobs that have not begun are discarded; already
applied focus/input changes are **not rolled back**, so don't blindly retry mutations.

## Trust, limits, and lifecycle

- Access grants UI control and visibility into script logs, live UI text, and terminal pixels. Treat it as access
  to the app, including any HTTP/process/filesystem permissions that app holds. Never expose
  the socket to untrusted users or forward it over a network. Secure input text is redacted
  by Rune; script logs are not secret-filtered.
- `DIR` is created with mode `0700` if missing (its parent must exist). An existing directory
  must be owned by the current user, exactly mode `0700`, and not a symlink. Socket mode is
  `0600`. Existing socket paths are refused, never removed to take over another process.
- One client is serviced at a time. Requests are at most 64 KiB including newline; responses
  at most 4 MiB. Reads, UI processing, and writes each have a two-second deadline. Slow or
  disconnected clients do not block the UI thread. Only one UI job may remain outstanding.
- Text is limited to 16 KiB, target keys to 512 bytes, coordinates to ±1,000,000, inventories
  to 10,000 entries. These and the recorder's approximate 1 MiB history budget are not a
  strict bound on all temporary allocations: Rune retains at least its latest frame.
- Socket I/O and JSON transport run on a worker; Rune inspection and input stay on the UI
  thread; image compression stays on the worker. Normal shutdown removes only the socket this
  server created and leaves `DIR`.
  A crash may leave a stale socket: use a new directory or remove the old socket only after
  verifying that its process has ended.
