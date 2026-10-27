-- A bar surface declared in Lua. Widgets are `riced.widget` instances, so the
-- widget files keep their own VMs, services, popups, and action handlers.
local layer = {}

local function widgets(list, prefix)
    local items = {}
    for index, name in ipairs(list or {}) do
        items[#items + 1] = riced.widget(name, {}, prefix .. "-" .. name .. "-" .. index)
    end
    return items
end

function layer:view(props)
    local edge = props.edge or "top"
    local thickness = props.thickness or 34
    local horizontal = edge == "top" or edge == "bottom"
    local anchors = { [edge] = true }
    if horizontal then
        anchors.left, anchors.right = true, true
    else
        anchors.top, anchors.bottom = true, true
    end
    local regions = props.widgets or {}
    local content = {
        iced.row(widgets(regions.start, "start")):spacing(8),
        iced.space():width(size.fill),
        iced.row(widgets(regions.center, "center")):spacing(8),
        iced.space():width(size.fill),
        iced.row(widgets(regions["end"], "end")):spacing(8),
        iced.button("Panel", "panel"),
        iced.button("Settings", "settings"),
    }
    local popup_y = edge == "top" and thickness or 0
    return iced.layer({
        id = props.id,
        layer = "top",
        anchors = anchors,
        exclusive_zone = thickness,
        width = not horizontal and thickness or nil,
        height = horizontal and thickness or nil,
        pointer_events = true,
        popup = riced.widget_popup({ x = 0, y = popup_y }),
        content = iced.container(
            horizontal and iced.row(content):spacing(12):align_y("center")
                or iced.column(content):spacing(12)
        ):padding(6):background(theme.surface):width(size.fill):height(size.fill),
    })
end

return layer
