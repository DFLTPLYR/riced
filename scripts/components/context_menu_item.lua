-- riced:composable
local app = { defaults = { padding = 5, rounding = 3 } }

function app:view(props)
    local button = ui.button(props.label, props.action):width(props.width or "fill")
        :padding(props.padding):radius(props.rounding)
    if props.color then button = button:color(props.color) end
    if props.background then button = button:background(props.background) end
    return button
end

return app
