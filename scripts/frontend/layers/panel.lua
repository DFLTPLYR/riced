-- Lua-owned bar panel, opened from the bar's Panel button. It reads and edits
-- `bars.lua` through root.lua, so there is no native Panel page and no IPC.
local layer = {}

function layer:view(props)
    local rows = {
        iced.text("Panel"):size(20):color(theme.on_surface),
        iced.text("Bars declared in bars.lua"):size(12):wrap("word"):color(theme.on_surface),
        iced.separator(),
    }
    local bars = props.bars or {}
    if #bars == 0 then
        table.insert(rows, iced.text("No bars. Add one below."):size(13):color(theme.on_surface))
    end
    for _, bar in ipairs(bars) do
        local edge = bar.edge or "top"
        table.insert(rows, iced.row({
            iced.text(edge .. " · " .. tostring(bar.thickness or 34) .. "px")
                :width(size.fill):color(theme.on_surface),
            iced.button("Remove", "panel:remove:" .. edge):padding(6),
        }):spacing(12):width(size.fill))
    end
    table.insert(rows, iced.row({
        iced.button("Add top", "panel:add:top"):padding(6),
        iced.button("Add bottom", "panel:add:bottom"):padding(6),
        iced.button("Add left", "panel:add:left"):padding(6),
        iced.button("Add right", "panel:add:right"):padding(6),
    }):spacing(8))
    table.insert(rows, iced.button("Close", "panel:close"):padding(6))
    return iced.layer({
        id = "panel",
        layer = "overlay",
        anchors = { top = true, right = true, bottom = true, left = true },
        exclusive_zone = -1,
        pointer_events = true,
        content = iced.container(
            iced.container(iced.column(rows):spacing(12))
                :padding(20):background(theme.surface):width(360)
        ):width(size.fill):height(size.fill):align_x("center"):align_y("center"),
    })
end

return layer
