-- Run with --allow-http --allow-process --allow-fs. Nothing external runs until clicked.
local ui = require("@rune/ui")
local http = require("@pane/http")
local process = require("@pane/process")
local fs = require("@pane/fs")
local json = require("@pane/json")

local state = { output = "Choose an action. Network and commands only run when clicked.", busy = false }
local activeProcess = nil

local function must(value, err)
    if value == nil then error(if err then err.message else "unexpected EOF", 2) end
    return value
end

local function append(text)
    state.output ..= text
    if #state.output > 65536 then state.output = string.sub(state.output, -65536) end
end

local function action(label, fn)
    return ui.row({
        key = label, px = 12, h = 28, radius = 14, align = "center", fill = "accent",
        hover = "accent-hover", focusable = true, role = "button", disabled = state.busy,
        onPress = function()
            if state.busy then return end
            state.busy = true
            state.output = ""
            local ok, err = pcall(fn)
            if not ok then
                append(`\nError: {err}\n`)
                if activeProcess then activeProcess:kill() end
            end
            activeProcess = nil
            state.busy = false
        end,
    }, { ui.text(label, { size = 12, color = "on-accent", weight = "medium" }) })
end

local function request()
    local response = must(http.request("https://api.github.com/repos/luau-lang/luau", {
        headers = { Accept = "application/vnd.github+json", ["User-Agent"] = "pane-script-demo" },
    }))
    append(`HTTP {response.status}\n`)
    append(must(response.body:readAll({ maxBytes = 1024 * 1024 })))
    response.body:close()
end

local function repository()
    local proc = must(process.spawn("gh", { "repo", "view", "luau-lang/luau", "--json", "nameWithOwner,url,description" }))
    activeProcess = proc
    local result = must(proc:collect())
    append(result.stdout)
    append(result.stderr)
    append(`\nexit: {result.exitCode}, signal: {result.signal}\n`)
end

local function files()
    append(must(json.encode(must(fs.readDir(".")))))
    append("\n\nFirst 240 bytes of this script:\n")
    local file = must(fs.open("init.luau"))
    append(must(file:read(240)))
    file:close()
end

local function streaming()
    -- Explicit shell for a fixed demo command. process.spawn itself never invokes a shell.
    local proc = must(process.spawn("/bin/sh", {
        "-c", "for i in 1 2 3 4 5; do printf 'output %s\n' \"$i\"; printf 'error %s\n' \"$i\" >&2; sleep 0.3; done",
    }))
    activeProcess = proc
    local finished = 0
    local function drain(reader, label)
        while true do
            local chunk, err = reader:read()
            if err then append(`[{label}] {err.message}\n`); break end
            if chunk == nil then break end
            append(`[{label}] {chunk}`)
        end
        finished += 1
    end
    task.spawn(drain, proc.stdout, "stdout")
    task.spawn(drain, proc.stderr, "stderr")
    local status, err = proc:wait()
    while finished < 2 do task.wait(0.01) end
    must(status, err)
    append(`\nexit: {status.exitCode}, signal: {status.signal}\n`)
end

return {
    render = function()
        return ui.col({ w = "full", h = "full", pad = 20, gap = 14, fill = "bg" }, {
            ui.text("Host I/O playground", { size = 20, weight = "semibold", color = "text" }),
            ui.text("HTTP bodies, files, stdout and stderr are byte readers.", { size = 12, color = "text-secondary" }),
            ui.row({ gap = 8 }, { action("HTTP", request), action("gh repo", repository), action("Files + JSON", files) }),
            ui.row({ gap = 8 }, {
                action("Stream both pipes", streaming),
                ui.row({ px = 12, h = 28, radius = 14, align = "center", fill = "control-off", hover = "hover", role = "button", focusable = true,
                    onPress = function() if activeProcess then activeProcess:kill() end end,
                }, { ui.text("Kill process", { size = 12, color = "text" }) }),
                ui.text(if state.busy then "Running…" else "Ready", { size = 12, color = "text-secondary" }),
            }),
            ui.col({ grow = 1, w = "full", scrollY = true, pad = 14, radius = 14, fill = "card" }, {
                ui.text(state.output, { size = 12, mono = true, color = "text", selectable = true }),
            }),
        })
    end,
    saveHotState = function() return { output = state.output } end,
    loadHotState = function(saved) state.output = saved.output or "Ready" end,
}
