-- Native capabilities: only opaque handles and byte-preserving results cross the bridge.
local native = ...
local pending = setmetatable({}, { __mode = "k" })
local originalCancel = task.cancel

-- Cancelling a Luau coroutine alone does not cancel host work. Connect the two explicitly.
task.cancel = function(thread)
    local ticket = pending[thread]
    if ticket then
        native.cancel(ticket)
        pending[thread] = nil
    end
    originalCancel(thread)
end

local function call(operation, args)
    if not coroutine.isyieldable() then
        error("host I/O must run in a task or event handler, not render", 3)
    end
    local ticket, err = native.newJob()
    if not ticket then return nil, err end
    local thread = coroutine.running()
    pending[thread] = ticket
    local ok, failure = pcall(native.await, ticket, operation, args)
    pending[thread] = nil
    if not ok then
        native.cancel(ticket)
        error(failure, 2)
    end
    return native.take(ticket)
end

local function options(value)
    return if value == nil then {} else table.clone(value)
end

local function reader(handle, owner)
    -- Keeping a process stream alive also keeps its owning process handle alive.
    local lifetime = { handle, owner }
    return table.freeze({
        read = function(_, size)
            return call("read", { handle = lifetime[1], size = size })
        end,
        readAll = function(_, opts)
            local args = options(opts)
            args.handle = lifetime[1]
            return call("readAll", args)
        end,
        close = function(_)
            return native.closeReader(lifetime[1])
        end,
    })
end

local function writer(handle, owner)
    local lifetime = { handle, owner }
    return table.freeze({
        write = function(_, bytes)
            return call("write", { handle = lifetime[1], bytes = bytes })
        end,
        close = function(_)
            return call("closeWriter", { handle = lifetime[1] })
        end,
    })
end

local http = {}
function http.request(url, opts)
    local args = options(opts)
    args.url = url
    local response, err = call("request", args)
    if not response then return nil, err end
    response.body = reader(response.body)
    return response, nil
end

local process = {}
function process.spawn(command, argv, opts)
    local args = options(opts)
    args.command = command
    args.args = argv or {}
    local result, err = call("spawn", args)
    if not result then return nil, err end
    local handle = result.handle
    return table.freeze({
        stdout = reader(result.stdout, handle),
        stderr = reader(result.stderr, handle),
        stdin = if result.stdin then writer(result.stdin, handle) else nil,
        wait = function(_)
            return call("wait", { handle = handle })
        end,
        collect = function(_, opts)
            local args = options(opts)
            args.handle = handle
            return call("collect", args)
        end,
        kill = function(_)
            return native.kill(handle)
        end,
    }), nil
end

local fs = {}
function fs.open(path, opts)
    local args = options(opts)
    args.path = path
    local handle, err = call("open", args)
    if not handle then return nil, err end
    return if args.mode == nil or args.mode == "read" then reader(handle) else writer(handle)
end
function fs.stat(path) return call("stat", { path = path }) end
function fs.readDir(path) return call("readDir", { path = path }) end
function fs.mkdir(path, opts)
    local args = options(opts)
    args.path = path
    return call("mkdir", args)
end
function fs.rename(from, to) return call("rename", { from = from, to = to }) end
function fs.remove(path) return call("remove", { path = path }) end

return { http = table.freeze(http), process = table.freeze(process), fs = table.freeze(fs) }
