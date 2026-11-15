-- Stable fixture: the live playground can change without changing the reload/input contract.
local ui = require("@rune/ui")
local count = 0
return {
    render = function()
        return ui.col({ gap = 12 }, {
            ui.text("A scriptable Pane"),
            ui.text(`Count: {count}`),
            ui.text("• Save to reload"),
            ui.row({ key = "increment", focusable = true, role = "button", onPress = function()
                count += 1
            end }, { ui.text("Increment") }),
            ui.row({ key = "reset", focusable = true, role = "button", onPress = function()
                count = 0
            end }, { ui.text("Reset") }),
        })
    end,
    saveHotState = function() return { count = count } end,
    loadHotState = function(saved) count = saved.count or 0 end,
}
