-- A dockable bar layer. `edge` chooses the screen edge this instance reserves.
-- The app declares one of these per entry in its own bar list, so one file
-- produces many surfaces with different ids, edges and thicknesses.
local layer = {}

function layer:view(props)
    local edge = props.edge or "top"
    local thickness = props.thickness or 32
    local horizontal = edge == "top" or edge == "bottom"
    local anchors = { [edge] = true }
    if horizontal then
        anchors.left, anchors.right = true, true
    else
        anchors.top, anchors.bottom = true, true
    end
    -- One axis is sized by thickness; the other fills between opposite anchors.
    local height, width
    if horizontal then
        height = thickness
    else
        width = thickness
    end
    return iced.layer({
        id = props.id or "bar",
        layer = "top",
        anchors = anchors,
        height = height,
        width = width,
        exclusive_zone = thickness,
        keyboard = "none",
        input = "all",
        content = iced.container(iced.row({
            iced.text("Bar · " .. props.output .. " · " .. edge):color(theme.on_surface),
            iced.space():width(size.fill),
            iced.text(props.clock or ""):color(theme.on_surface),
        }):spacing(12):height(size.fill):align_y("center"))
            :padding({ left = 10, right = 10 })
            :background(theme.surface):width(size.fill):height(size.fill),
    })
end

return layer
