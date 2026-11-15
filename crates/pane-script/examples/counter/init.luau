--!strict
-- Save this file while pane-script is running: it reloads in place and keeps the count
-- (saveHotState / loadHotState).
local ui = require("@rune/ui")

local state = { count = 0 }

local function button(key: string, label: string, onPress: () -> ())
	return ui.row({
		key = key,
		px = 12,
		py = 6,
		radius = 6,
		fill = "control-off",
		hover = "hover",
		press = "press",
		role = "button",
		label = label,
		focusable = true,
		onPress = onPress,
	}, { ui.text(label, { weight = "medium" }) })
end

local function render(view)
	local items = {}
	for i, item in { "Edit this file", "Save to reload", "Keep counting" } do
		table.insert(items, ui.text(`• {item}`, { key = `tip-{i}` }))
	end
	return ui.col({ key = "page", w = "full", h = "full", pad = 24, gap = 12, fill = "bg" }, {
		ui.text("A scriptable Pane", { size = 15, weight = "semibold" }),
		ui.text(`Count: {state.count}`, { key = "count" }),
		ui.row({ gap = 8 }, {
			button("increment", "Increment", function()
				state.count += 1
			end),
			button("reset", "Reset", function()
				state.count = 0
			end),
		}),
		ui.col({ gap = 4 }, items),
		ui.text("Click a button, or Tab then Space. The count survives a saved edit.", { color = "text-secondary" }),
	})
end

return {
	render = render,
	saveHotState = function()
		return state
	end,
	loadHotState = function(saved)
		state.count = saved.count or 0
	end,
}
