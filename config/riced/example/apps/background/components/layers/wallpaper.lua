-- The wallpaper layer. Owns its whole surface: background level, full-output
-- anchors, pointer events for selection. The app passes state in; this file
-- decides what the surface looks like.
local layer = {}

local function stack_of(props)
    local output = props.output
    local layers = {
        iced.container(iced.space()):width(size.fill):height(size.fill)
            :background(props.color),
    }
    if props.wallpaper and props.wallpaper ~= "" then
        -- iced.image currently uses native contain fitting, not wallpaper cover.
        layers[#layers + 1] = iced.image(props.wallpaper):width(size.fill):height(size.fill)
    end
    -- Host-owned desktop widgets (`desktop` placements in config.lua). The
    -- native host owns instances, service snapshots, hold-drag and persistence.
    layers[#layers + 1] = iced.native("desktop-widgets")
    layers[#layers + 1] = iced.container(iced.column({
        iced.text("Lua background · " .. output.name):size(24),
        iced.text("Right-click for wallpaper options · drag to select"),
        iced.button(props.picking and "Picker open…" or "Choose wallpaper", "pick-wallpaper"),
        iced.text(props.error or "Preferences reload automatically when edited")
            :color(props.error and theme.error or theme.on_surface),
    }):spacing(10)):padding(24):anchor("top_left")

    local s = props.selection
    if s and s.width > 0 and s.height > 0 then
        layers[#layers + 1] = iced.container(iced.space())
            :width(s.width):height(s.height)
            :background("#89b4fa22"):border("#89b4fa"):border_width(1)
            :anchor("top_left"):margin({ left = s.x, top = s.y })
            :animate({ id = "selection", opacity = iced.animations.opacity({
                to = props.selecting and 1 or 0, duration = props.fade_ms,
            }) })
    end
    if props.menu then
        -- Reserve enough room for five rows; constrain to small outputs too.
        local width = math.min(props.menu_width, output.width)
        local height = math.min(190, output.height)
        local x = math.max(0, math.min(props.menu.x, output.width - width))
        local y = math.max(0, math.min(props.menu.y, output.height - height))
        layers[#layers + 1] = riced.component("menu.lua", { width = width, height = height })
            :anchor("top_left"):margin({ left = x, top = y })
    end
    return layers
end

function layer:view(props)
    return iced.layer({
        id = "background",
        layer = "background",
        anchors = { top = true, right = true, bottom = true, left = true },
        exclusive_zone = -1,
        keyboard = "none",
        input = "all",
        pointer_events = true,
        content = iced.stack(stack_of(props)):width(size.fill):height(size.fill),
    })
end

return layer
