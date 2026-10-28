-- Rust supplies palette metadata, cached icon tokens, and allowed action keys.
local app = {}

local function heading(title, description)
    return iced.column({
        iced.text(title):size(16):color(theme.on_surface),
        iced.text(description):size(12):wrap("word"):color(theme.on_surface),
    }):spacing(5):width(size.fill)
end

local function swatch(color, outline)
    return iced.container(iced.space()):width(24):height(24)
        :background(color):border(outline):radius(4)
end

local function choice(label, action, selected)
    return iced.button(label, action):width(size.fill):padding(8)
        :background(selected and theme.primary or theme.surface)
        :color(selected and theme.on_primary or theme.on_surface)
end

function app:view(props)
    local children = {
        heading("Color mode", "Current palette: " .. props.name .. " · " .. (props.darkmode and "Dark" or "Light")),
        iced.row({
            choice("Dark", "mode:dark", props.darkmode),
            choice("Light", "mode:light", not props.darkmode),
        }):spacing(8):width(size.fill),
        heading("Icon theme", "Used for application, tray, and notification icons. Missing icons fall back to hicolor."),
    }
    if props.preview_status ~= "ready" then
        table.insert(children, iced.text(props.preview_message):size(13):wrap("word"))
    else
        local cards = {}
        for _, item in ipairs(props.icon_themes) do
            local selected = item.name == props.icon_theme
            local samples = {}
            for start = 1, 6, 3 do
                local cells = {}
                for i = start, start + 2 do
                    local sample = item.samples[i]
                    local image
                    if type(sample.image) == "string" and props.images and props.images[sample.image] then
                        image = iced.image_handle(props.images[sample.image]):width(40):height(40)
                    else
                        image = iced.container(iced.text("?"):size(24)):width(40):height(40)
                            :align_x("center"):align_y("center")
                    end
                    table.insert(cells, iced.column({
                        iced.container(image):width(size.fill):align_x("center"),
                        iced.text(sample.label):size(10):width(size.fill):align_x("center"),
                    }):spacing(4):width(size.fill))
                end
                table.insert(samples, iced.row(cells):spacing(12):width(size.fill))
            end
            table.insert(cards, iced.container(iced.column({
                iced.row({
                    iced.text(item.name):size(13):width(size.fill):height(36),
                    iced.text(selected and "Selected" or ""):size(10),
                }):spacing(8),
                iced.column(samples):spacing(10):width(size.fill),
            }):spacing(12)):padding(12):width(size.fill):radius(8)
                :background(theme.surface):border(selected and theme.primary or theme.outline)
                :border_width(selected and 2 or 1):on_press("icon:" .. item.name))
        end
        table.insert(children, iced.masonry(cards):column_width(260):spacing(12))
    end
    table.insert(children, heading("Choose a palette", "Swatches preview the primary, secondary, and tertiary colors."))
    for _, palette in ipairs(props.palettes) do
        local selected = palette.name == props.name
        table.insert(children, iced.container(iced.row({
            iced.text(palette.name):size(13):color(palette.on_surface),
            iced.text(selected and "Selected" or ""):size(11):color(palette.on_surface),
            iced.space():width(size.fill),
            iced.row({
                swatch(palette.primary, palette.outline),
                swatch(palette.secondary, palette.outline),
                swatch(palette.tertiary, palette.outline),
            }):spacing(4),
        }):spacing(8):width(size.fill)):width(size.fill):padding(14):radius(8)
            :background(palette.surface):border(selected and palette.primary or palette.outline)
            :border_width(selected and 2 or 1):on_press("palette:" .. palette.name))
    end
    return iced.column(children):spacing(16):width(size.fill)
end

return app
