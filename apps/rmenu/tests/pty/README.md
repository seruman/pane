# Repository checks

Run `./check.sh` from any working directory. It builds the workspace, runs the
Rust tests and Clippy, runs the example checks, and runs these PTY tests.
The example checks require fish and jq. The PTY tests require Python 3.9 or
newer, a Unix host with POSIX shared memory, and the dependencies needed to
build Rune. The workspace's Rune dependency must be available before building.

The script uses two Cargo jobs, disables incremental compilation and debug
information, and builds in `target/check`. Set `CARGO_TARGET_DIR` to use another
directory. Set `RMENU_BIN` to test another binary after the workspace checks.
To run just the PTY tests against an existing binary:

```sh
python3 -I tests/pty/check.py --binary /path/to/rmenu
```

Without `--binary`, the runner uses `RMENU_BIN` or `target/check/debug/rmenu`.
Paths inside the tests are relative to the scripts, not the caller's directory.
The runner accepts unittest filters, for example
`PickerTests.test_typing_and_delayed_shared_memory`.

The tests cover line and NUL selection, JSON input and output, malformed input,
cancellation, caller environment preservation, and refusal to run without a
controlling terminal. Repeated empty selections must redraw without closing the
picker or writing selection output. Escape, Ctrl-C, SIGTERM, SIGHUP and SIGINT
interrupt a streaming source while it is still loading. Each interruption test
checks that the source was reaped and that the exit status and output match the
expected outcome.

The terminal answers startup and Kitty graphics queries. It checks payload
sizes and keeps only frame metadata and SHA-256 digests. Typing must change a
frame and select the filtered row. A 240 by 128 pixel terminal exercises the
inline transport and a small layout. A separate shared-memory run requires
actual frame edits and opens their objects 50 ms after parsing their commands.
It holds fence replies until those reads finish. An object unlinked before the
read fails the test, rather than silently dropping the frame. An injected
early unlink checks that this failure path works.

These are protocol and outcome checks, not pixel-reference tests. They do not
prove visual layout correctness or detect every possible flash. They never
write screenshots or image artifacts and never call Rex. The example checks
use their own fake Rex executable.

Every picker gets a private child PTY. The runner redirects selection output
and diagnostics to temporary files. Waits have deadlines. Cleanup kills only
the picker process group and the source group recorded by the test helper,
then reaps the picker and closes its PTY. The source stops after 30 seconds even
if the test runner disappears. An injected assertion failure tests picker and
PTY cleanup. The environment removes terminal debug and trace settings so a
caller's settings cannot create capture artifacts.

## Provenance

`terminal.py` adapts the terminal query and graphics handling from Rune commit
`1855d968`, file `.auto/fake_terminal.py`, Git blob
`f4b3473ba0bb6bcfb3b5d76f959df18d25991703`. `LICENSE.rune` preserves the upstream
license and copyright notice from that commit. It applies to the adapted
harness, not to rmenu's production code.

The cases adapt the review's `rmenu-terminal-api.py`, `rmenu-host-quit.py`,
`rmenu-outcomes.py` and `shmlag.py` probes. They replace external paths and data
with local fixtures. The async check uses a timed queue instead of reading
shared memory during command parsing. Capture, benchmark and real-terminal
entry points from the original harness are not included.
