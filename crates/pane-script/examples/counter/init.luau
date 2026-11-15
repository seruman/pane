-- Live task board. Edit this file while the app is running: no host restart needed.
local ui = require("@rune/ui")
local lanes = { "Now", "Next", "Done" }
local templates = {
    "Polish the empty state",
    "Check keyboard navigation",
    "Ship the hot reload demo",
    "Write release notes",
}
local state = {
    count = 0, imported = 0, nextId = 4, selected = 1,
    composer = false, draft = 1, done = {},
    tasks = {
        { id = 1, title = "Shape the live board", lane = "Now", credited = false },
        { id = 2, title = "Check keyboard navigation", lane = "Next", credited = false },
        { id = 3, title = "Write release notes", lane = "Next", credited = false },
    },
}

local function label(text, key, size, color)
    return ui.text(text, {
        key = key, size = size or 12, color = color or "text-secondary",
    })
end

local function button(key, title, fn, primary, disabled)
    return ui.row({
        key = key, h = 30, px = 12, gap = 6, radius = 15, center = true,
        fill = if primary then "accent" else "control-off",
        hover = if primary then "accent-hover" else "hover", press = "press",
        role = "button", label = title, focusable = true, disabled = disabled or false,
        onPress = fn,
    }, {
        ui.text(title, { size = 12, weight = "medium", color = if primary then "on-accent" else "text" }),
    })
end

local function move(item, lane)
    if lane == "Done" and not item.credited then
        state.count += 1
        item.credited = true
    end
    item.lane = lane
    state.selected = item.id
end

local function addTask()
    if #state.tasks >= 60 then return end
    local id = state.nextId
    table.insert(state.tasks, { id = id, title = templates[state.draft], lane = "Next", credited = false })
    state.nextId += 1
    state.selected = id
    state.composer = false
end

local function card(item)
    local selected = item.id == state.selected
    local destination = if item.lane == "Next" then "Now" elseif item.lane == "Now" then "Done" else "Next"
    local action = if item.lane == "Next" then "Start →" elseif item.lane == "Now" then "Ship ✓" else "Reopen"
    return ui.col({
        key = `card-{item.id}`, w = "full", pad = 14, gap = 14, radius = 12,
        fill = "bg", borderWidth = 1, border = if selected then "accent" else "separator",
    }, {
        ui.col({
            key = `select-{item.id}`, w = "full", gap = 7, radius = 4,
            focusable = true, role = "button", label = `Select {item.title}`,
            hover = "hover", onPress = function() state.selected = item.id end,
        }, {
            label(`TASK {string.format("%02d", item.id)}`, nil, 10),
            ui.text(item.title, { key = `title-{item.id}`, size = 14, weight = "medium", color = "text" }),
        }),
        ui.row({ w = "full", align = "center", gap = 8, wrap = true }, {
            label(if selected then "Selected" elseif item.credited then "Shipped once" else "Ready to go", `status-{item.id}`, 11),
            ui.spacer(),
            button(`move-{item.id}`, action, function() move(item, destination) end, item.lane == "Now"),
        }),
    })
end

local function column(lane)
    local cards = {}
    for _, item in state.tasks do
        if item.lane == lane then table.insert(cards, card(item)) end
    end
    local n = #cards
    if n == 0 then
        table.insert(cards, ui.col({ w = "full", minH = 110, center = true, gap = 6 }, {
            label(if lane == "Done" then "The next small win goes here." else "Room for something good.", nil, 12),
        }))
    end
    return ui.col({
        key = `lane-{lane}`, w = 0, grow = 1, minW = 180, minH = 250,
        pad = 12, gap = 12, radius = 16, fill = "card",
    }, {
        ui.row({ w = "full", px = 2, py = 4, gap = 8, align = "center" }, {
            ui.box({ w = 6, h = 6, radius = 3, fill = "accent", opacity = if lane == "Now" then 1 else 0.4 }),
            ui.text(lane, { size = 14, weight = "semibold", color = "text" }),
            ui.spacer(),
            label(tostring(n), `total-{lane}`, 12),
        }),
        ui.col({ w = "full", gap = 10 }, cards),
    })
end

local function progress()
    local done = 0
    for _, item in state.tasks do
        if item.lane == "Done" then done += 1 end
    end
    local value = if #state.tasks > 0 then done / #state.tasks else 0
    return ui.col({ key = "progress", w = "full", gap = 8 }, {
        ui.row({ w = "full", align = "center", gap = 8 }, {
            label("THIS BOARD", nil, 10),
            ui.spacer(),
            label(`{done} / {#state.tasks} complete · {math.floor(value * 100)}%`, "board-progress", 12),
        }),
        ui.row({ w = "full", h = 4, radius = 2, clip = true, fill = "control-off" }, {
            if value > 0 then ui.box({ grow = value, h = 4, radius = 2, fill = "accent" }) else nil,
            if value < 1 then ui.box({ grow = 1 - value, h = 4 }) else nil,
        }),
    })
end

local function composer()
    local choices = {}
    for i, title in templates do
        table.insert(choices, button(`template-{i}`, title, function() state.draft = i end, state.draft == i))
    end
    return ui.col({ key = "composer", w = "full", pad = 16, gap = 12, radius = 14, fill = "card" }, {
        label("Pick a task template · new cards start in Next", nil, 12),
        label(templates[state.draft], "draft-title", 14, "text"),
        ui.row({ w = "full", gap = 8, wrap = true }, choices),
        ui.row({ gap = 8 }, {
            button("confirm-add", "Add to Next", addTask, true, #state.tasks >= 60),
            button("cancel-add", "Cancel", function() state.composer = false end),
        }),
    })
end

local function selection()
    local selected = nil
    for _, item in state.tasks do
        if item.id == state.selected then selected = item; break end
    end
    if not selected then return nil end
    local item = selected
    local actions = {}
    for _, lane in lanes do
        table.insert(actions, button(`send-{lane}`, lane, function() move(item, lane) end, item.lane == lane, item.lane == lane))
    end
    return ui.row({
        key = "selection", w = "full", pad = 16, gap = 16, radius = 14,
        borderWidth = 1, border = "separator", align = "center", wrap = true,
    }, {
        ui.col({ grow = 1, minW = 150, gap = 4 }, {
            label("SELECTED TASK", nil, 10),
            ui.text(item.title, { key = "selected-title", size = 14, weight = "medium", color = "text" }),
            label(`Task {item.id} · {item.lane}`, "selected-lane", 12),
        }),
        ui.row({ gap = 8, align = "center", wrap = true }, {
            label("Move to", nil, 12),
            ui.row({ gap = 6 }, actions),
        }),
    })
end

return {
    render = function()
        return ui.col({ key = "page", w = "full", h = "full", fill = "bg", scrollY = true }, {
            ui.col({ w = "full", pad = 24, gap = 18 }, {
                ui.row({ w = "full", gap = 14, align = "center", wrap = true }, {
                    ui.col({ grow = 1, minW = 180, gap = 6 }, {
                        label("PANE / LIVE WORKSPACE", nil, 10),
                        ui.text("Ship small. Stay live.", { key = "board-title", size = 28, weight = "semibold", color = "text" }),
                        label("Small tasks. Real progress. Same running app.", nil, 12),
                    }),
                    ui.col({ px = 18, py = 10, gap = 3, radius = 14, fill = "card", center = true }, {
                        ui.text(tostring(state.count), { key = "count", size = 26, weight = "semibold", color = "accent-text", tabular = true }),
                        label("TASKS SHIPPED", nil, 10),
                    }),
                    button("add-task", "+ Add task", function() state.composer = not state.composer end, true),
                }),
                if state.composer then composer() else nil,
                progress(),
                ui.row({ key = "board", w = "full", gap = 12, align = "stretch", wrap = true }, {
                    column("Now"), column("Next"), column("Done"),
                }),
                selection(),
                label(`{state.imported} carried from the counter · each task counts once when shipped`, "history", 11),
                label("Click a card to select it. Tab + Space works too. Tasks, selection and the open picker survive code edits.", nil, 11),
            }),
        })
    end,
    saveHotState = function() return state end,
    loadHotState = function(saved)
        if type(saved.tasks) == "table" then
            state = saved
        else
            -- Migrate the counter's live state instead of resetting it.
            state.count = saved.count or 0
            state.imported = state.count
            state.done = saved.done or {}
        end
    end,
}
