-- The root owns commands and shared state. The established Rust hosts retain
-- bar slots/editing, widget popups, desktop gestures and notification behavior.
-- Add custom Lua surfaces here without replacing those hosts with demo layers.
local state = require("state")

return {
    view = function(self, props)
        return nil
    end,
}
