-- M0 declarative app. One app table/environment owns this instance's state.
-- Lua returns descriptions; Rust owns cached IR and rebuilds iced Elements.
local app = {}

function app:view(window_id)
    return ui.container(ui.column({
        ui.text("hello from main.lua"):size(24),
        ui.text("This window is built from a Lua description tree."),
        ui.text("Window " .. window_id),
    }):spacing(12)):padding(24):width("fill"):height("fill")
end

return app
