-- riced:composable
-- App-backed shell component. Rust supplies width/height and drag state;
-- appearance defaults and styling live here. Read theme inside view.
local app = {
    defaults = { radius = 0, border_width = 1, fill_alpha = 0.5 },
}

local function alpha(hex, opacity)
    return hex:sub(1, 7) .. string.format("%02x", math.floor(math.max(0, math.min(1, opacity)) * 255 + 0.5))
end

function app:view(props)
    local color = props.color or theme.primary
    return ui.container(ui.space())
        :width(props.width):height(props.height)
        :background(alpha(color, props.fill_alpha))
        :border(color):border_width(props.border_width):radius(props.radius)
end

return app
