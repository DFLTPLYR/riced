local layer = {}
function layer:view(props)
    local horizontal = props.edge == "top" or props.edge == "bottom"
    local anchors = { [props.edge]=true }
    if horizontal then anchors.left, anchors.right = true, true
    else anchors.top, anchors.bottom = true, true end
    local items = {
        iced.button("Settings", "settings"),
        iced.text(props.clock):color(theme.on_surface),
    }
    local content
    if horizontal then content=iced.row(items):spacing(12):align_y("center")
    else content=iced.column(items):spacing(12) end
    local popup
    if props.menu then
        local x, y = props.menu.x, props.menu.y
        if props.edge == "bottom" then y = props.output.height - props.thickness + y - 52
        elseif props.edge == "right" then x = props.output.width - props.thickness + x - 168
        elseif props.edge == "top" then y = y + props.thickness
        else x = x + props.thickness end
        popup = {
            id="menu", x=x, y=y, width=168, height=52,
            content=iced.container(iced.button("Remove bar", "remove:" .. props.edge):width(size.fill))
                :padding(8):background(theme.surface):width(size.fill):height(size.fill),
        }
    end
    return iced.layer({
        id=props.id, layer="top", anchors=anchors, exclusive_zone=props.thickness,
        width=not horizontal and props.thickness or nil,
        height=horizontal and props.thickness or nil,
        pointer_events=true, popup=popup,
        content=iced.container(content):padding(6):background(theme.surface)
            :width(size.fill):height(size.fill),
    })
end
return layer
