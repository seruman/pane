# Native host APIs

These are Pane bindings, not Luau built-ins. JSON is always available; native I/O is opt-in:

```sh
cargo run -p pane-script -- crates/pane-script/examples/io \
  --allow-http --allow-process --allow-fs
```

Add `RUNE_TERMINAL=1` to run in the terminal. The I/O playground does nothing external until
an action is clicked. `gh repo` uses the installed `gh`, inherited environment and existing auth.

**Only grant capabilities to trusted scripts.** Filesystem access is not confined to the script
folder. Process access runs as your user and can itself access files and the network even when
`--allow-fs`/`--allow-http` are absent. These switches are capability gates, not an OS sandbox.
Missing permissions return `nil, { kind = "permission_denied", message = "..." }`.

## Calls and streams

Call I/O inside `task.spawn`, a UI event handler, or a signal handler. Calls suspend that task,
not the UI thread; no `await` keyword is needed. Calling I/O from render is an error.

```luau
local function must(value, err)
    assert(value ~= nil, if err then err.message else "unexpected nil")
    return value
end
```

Operations return `value, nil` on success or `nil, { kind, message }` on failure. Errors include
`invalid_argument`, `permission_denied`, `not_found`, `already_exists`, `io`, `timeout`, `limit`,
`busy`, `closed`, and `cancelled`. Misuse of the interface itself (e.g. calling outside a task)
raises a Luau error. Successful writes/closes/filesystem mutations return `true`.

**Reader** — the same interface for `response.body`, `proc.stdout`, `proc.stderr`, and read files:

```luau
local bytes, err = reader:read()                 -- up to 64 KiB; may return fewer bytes
local bytes, err = reader:read(4096)             -- size: 1..65536
local bytes, err = reader:readAll({ maxBytes = 4 * 1024 * 1024 })
reader:close()                                 -- aborts an in-flight read; idempotent
```

`read()` returns `nil, nil` at EOF, never an empty chunk. `readAll()` returns a string (possibly
empty) and collects only the unread remainder. Both preserve arbitrary bytes, including NUL
and invalid UTF-8. Chunks have no line, character, JSON, or SSE framing. Collection defaults to
4 MiB, configurable up to 16 MiB; exceeding the limit closes the reader and returns `limit`.
A pending read times out after 30 seconds without data. EOF releases the underlying source.

One reader operation at a time: concurrent reads/collection return `busy`, rather than stealing
bytes. Closing a process pipe is **not** discarding its output: the child may receive a broken
pipe. Drain unwanted output in a task, or use `collect()`.

**Writer** — piped stdin and write/append files:

```luau
local ok, err = writer:write(bytes) -- all bytes or error; at most 16 MiB per call
local ok, err = writer:close()     -- flush/shutdown, idempotent
```

Writes and close have a 30-second timeout. Always close file writers explicitly to observe
flush errors. Flush is not an `fsync` durability guarantee. An error or cancellation can occur
after a partial write; there is no rollback.

## HTTP: one request API

```luau
local http = require("@pane/http")
local json = require("@pane/json")

task.spawn(function()
    local response = must(http.request("https://example.com/api", {
        method = "POST", -- default GET
        headers = { ["Content-Type"] = "application/json" },
        body = must(json.encode({ hello = "world" })),
        timeoutMs = 15000,
        maxBodyBytes = 2 * 1024 * 1024,
    }))
    print(response.status, response.url)
    local bytes = must(response.body:readAll())
    local value = must(json.decode(bytes))
end)
```

`request(url, options?)` returns after response headers arrive. The response has:

- `status`: numeric HTTP status, including ordinary 4xx/5xx responses.
- `url`: final URL after redirects (at most five redirects).
- `headers`: lowercase names mapped to arrays of byte strings, preserving repeated headers.
- `body`: a Reader, always. Stream with `read()` or collect with `readAll()`.

Options: `method`, `headers` (values can be strings or arrays), `body` (bytes), `timeoutMs`
(default 30000, max 3600000), `maxBodyBytes` (default/max 16 MiB). The total request timeout
includes consuming the body, not just headers. The byte limit applies to streaming too.
Request bodies are limited to 16 MiB; headers are limited to 128 values and 64 KiB. Only HTTP(S)
URLs are supported. There is no application-level retry loop. Close unused bodies promptly.

## Processes: one spawn API, separate streams

```luau
local process = require("@pane/process")
local json = require("@pane/json")

task.spawn(function()
    local proc = must(process.spawn("gh", {
        "pr", "list", "--repo", "owner/repo", "--json", "number,title",
    }, { timeoutMs = 30000 }))
    local result = must(proc:collect())
    if result.exitCode == 0 then
        state.prs = must(json.decode(result.stdout))
    else
        state.error = result.stderr
    end
end)
```

`spawn(command, args?, options?)` executes the program directly; there is no implicit shell.
Options:

- `cwd`: script directory by default; relative overrides resolve against that directory.
- `env`: string-to-string overrides. Environment is inherited unless `inheritEnv = false`.
- `stdin`: `"null"` by default; `"pipe"` exposes `proc.stdin`, a Writer.
- `timeoutMs`: execution timeout; default 30000, max 3600000.

Process members:

- `stdout`, `stderr`: **independent Readers**. Drain both concurrently to avoid pipe backpressure.
- `stdin`: Writer when requested, otherwise nil. Close it when input is complete.
- `wait()`: waits for exit and returns `{ exitCode, signal }`. It does **not** read output.
- `collect({ maxBytes? })`: drains both streams concurrently with a **combined** byte limit,
  then waits. Returns `{ stdout, stderr, exitCode, signal }`. Defaults to 4 MiB, max 16 MiB.
  It collects only unread output; it must not overlap with manual reading. It does not close
  piped stdin for you. Limit failures terminate/reap the process before returning an error.
- `kill()`: requests SIGKILL of the active process group without blocking the UI; `wait()`
  observes completion. Safe to call repeatedly.

A nonzero exit is a result, not a host error. `exitCode` is nil for signal termination;
`signal` is the numeric signal, otherwise nil. Timeout/cancellation failures return errors.

Streaming:

```luau
task.spawn(function()
    local proc = must(process.spawn("gh", { "run", "watch", runId }))
    local function drain(reader, consume)
        while true do
            local chunk, err = reader:read()
            if err then state.error = err.message; break end
            if chunk == nil then break end
            consume(chunk)
        end
    end
    task.spawn(drain, proc.stdout, appendStdout)
    task.spawn(drain, proc.stderr, appendStderr)
    local status, err = proc:wait()
    -- The reader tasks can still be draining final buffered bytes after process exit.
end)
```

Pipes preserve order within each stream, not a total order between them. Some tools buffer
when not connected to a terminal. PTYs are not implemented. Process support currently requires
Unix. Managed processes should remain in the foreground: daemonized children that detach or
outlive an already-reaped leader are not tracked. Do not use this as a service supervisor.

## Filesystem

```luau
local fs = require("@pane/fs")
local file = must(fs.open("config.json", { mode = "read" }))
local text = must(file:readAll())
file:close()

local output = must(fs.open("output.txt", { mode = "write" }))
must(output:write(text))
must(output:close())
```

`open(path, { mode? })` supports `"read"` (default), `"write"` (create/truncate), and `"append"`
(create/append). It returns a Reader or Writer. Paths are UTF-8 strings; relative paths resolve
against the script directory, absolute paths are accepted. Closing files releases resource slots.

Other calls (all task-suspending):

- `stat(path)` → `{ type, size, modifiedMs? }`; like `lstat`, does not follow symlinks.
- `readDir(path)` → sorted `{ { name, type }, ... }`, at most 10,000 entries.
- `mkdir(path, { recursive? })` → true; recursive defaults to false.
- `rename(from, to)` → true; follows the platform's overwrite semantics.
- `remove(path)` → true; removes a file/link or **empty** directory, never recursively.

Types are `"file"`, `"directory"`, `"symlink"`, or `"other"`. File opening follows symlinks.
Non-UTF-8 directory entry names are displayed lossily; this initial API is for UTF-8 paths.

## JSON

Luau has no built-in JSON codec. `require("@pane/json")` exposes:

- `decode(text)` → value, or nil/error.
- `encode(value)` → text, or nil/error.
- `null`: sentinel preserving JSON null, including inside arrays/objects. Luau nil also encodes as null.
- `array(table)`, `object(table)`: mark and return the table, including empty `[]` versus `{}`.

Decoded containers retain their identity. Unmarked empty tables encode as objects; nonempty
tables must have either string keys or dense array keys. Cycles, mixed/sparse keys, unsupported
values, non-finite numbers, and invalid UTF-8 are errors. Integers outside ±(2^53−1) are rejected
rather than silently rounded. Fractional numbers use normal double precision. Documents are
limited to 1 MiB, 64 container levels and 100,000 values. JSON operations are synchronous and bounded.

## Lifetime and cancellation

There are at most 64 open resource groups and 64 outstanding calls per VM. Pipes share their
process's resource slot. Closed handles do not consume resource slots.

`task.cancel(thread)` cancels its pending host call. Cancelling a read/write closes that stream;
cancelling a process `wait`/`collect` kills its process. A process already returned by `spawn`
is otherwise owned by its handle: cancelling an unrelated `task.wait` does not kill it; use
`proc:kill()` explicitly. Dropped handles close their resources, but prefer explicit close/kill
over relying on garbage collection.

Successful hot reload/app shutdown cancels the old VM's I/O and kills/reaps active process
groups. Failed reload keeps the previous VM and its resources alive. Only plain application
data belongs in hot state; handles cannot transfer to a new VM. Filesystem cancellation is best
effort (Tokio uses blocking filesystem work internally), and completed side effects are not undone.
