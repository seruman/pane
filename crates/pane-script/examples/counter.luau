-- Save this file while pane-script is running. init receives the last good state.
return {
    init = function(previous)
        return previous or { count = 0 }
    end,

    update = function(state, message)
        if message == "increment" then
            state.count += 1
        elseif message == "reset" then
            state.count = 0
        end
        return state
    end,

    view = function(state)
        local items = {}
        for _, item in { "Edit this file", "Save to redraw", "Keep counting" } do
            table.insert(items, ui.text("• " .. item))
        end
        return ui.column({
            ui.text("A scriptable Pane"),
            ui.text("Count: " .. state.count),
            ui.row({
                ui.button("Increment", "increment"),
                ui.button("Reset", "reset"),
            }),
            ui.column(items),
            ui.text("Click a button, or Tab then Space. State survives a saved edit."),
        })
    end,
}
