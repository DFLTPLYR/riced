-- riced:composable
local app = {
    defaults = { width = 178, height = 92, auto_sizing = true, padding = 4, spacing = 5, rounding = 3, border_width = 1 },
}

function app:view(props)
    local width_mode = type(props.width) == "string" and string.lower(props.width) or ""
    local auto_width = width_mode == "auto" or width_mode == "shrink"
    local children = {}
    for _, item in ipairs(props.items or {}) do
        local item_props = { label = item.label, action = item.action, width = auto_width and "shrink" or "fill" }
        children[#children + 1] = riced.component(props.item_component.src, props.item_component.props, item_props)
    end
    return ui.container(ui.column(children):spacing(props.spacing):width(auto_width and "shrink" or "fill"))
        :width(props.width):height(props.auto_sizing and "shrink" or props.height):padding(props.padding):radius(props.rounding)
        :background(props.background or theme.surface)
        :border(props.border or theme.secondary):border_width(props.border_width)
end

return app
