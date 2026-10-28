-- Desktop Settings. Rust owns previews, discovery, and every config patch.
local app = {}

local function hint(text)
    return iced.text(text):size(12):wrap("word"):color(theme.on_surface)
end

local function heading(title)
    return iced.text(title):size(18):color(theme.on_surface)
end

local function choice(label, action, selected)
    return iced.button(label, action):padding(6)
        :background(selected and theme.primary or theme.surface)
        :color(selected and theme.on_primary or theme.on_surface)
end

function app:view(props)
    local rows = {}
    local holds = { iced.text("Hold duration"):width(size.fill) }
    for _, hold in ipairs(props.holds) do
        table.insert(holds, choice(hold.label, hold.action, hold.selected))
    end
    table.insert(rows, iced.row(holds):spacing(8):width(size.fill))
    if type(props.custom) == "table" then
        table.insert(rows, iced.row({
            iced.text("Milliseconds"):width(size.fill),
            iced.spin_box(props.custom, 1, 60000):step(25):decimals(0):on_change("hold:ms"):width(120),
        }):spacing(8):width(size.fill))
    end
    table.insert(rows, hint("Applies to widget holds, including desktop drag pickup."))
    table.insert(rows, heading("Available desktop widgets"))
    local displays = { iced.text("Preview display"):width(size.fill) }
    for _, display in ipairs(props.displays) do
        table.insert(displays, choice(display.label, display.action, display.selected))
    end
    table.insert(rows, iced.row(displays):spacing(8):width(size.fill))
    table.insert(rows, hint("Click inside a widget preview to pick it up at that point. Release, then click the desktop to place."))
    local available = {}
    for _, card in ipairs(props.available) do
        table.insert(available, iced.container(iced.column({
            iced.button("Place " .. card.name, card.action),
            riced.plugin(card.slot),
        }):spacing(8)):padding(12):width(size.fill))
    end
    table.insert(rows, iced.masonry(available):column_width(344):spacing(16))
    table.insert(rows, heading("Placed widgets"))
    local placed = {}
    for _, card in ipairs(props.placed) do
        table.insert(placed, iced.container(iced.column({
            riced.plugin(card.slot),
            iced.text(card.caption):size(13):width(size.fill),
            iced.row({
                iced.button("Reposition", card.reposition),
                iced.button("Remove", card.remove),
            }):spacing(8),
            iced.row({
                iced.text("X"),
                iced.spin_box(card.x, -32768, 32768):step(1):decimals(0):on_change(card.x_action),
                iced.text("Y"),
                iced.spin_box(card.y, -32768, 32768):step(1):decimals(0):on_change(card.y_action),
            }):spacing(8):width(size.fill),
            hint("Size overrides (0 = Lua)"),
            iced.row({
                iced.text("Width"):size(12),
                iced.spin_box(card.width, 0, 32768):step(1):decimals(0):on_change(card.width_action),
                iced.text("Height"):size(12),
                iced.spin_box(card.height, 0, 32768):step(1):decimals(0):on_change(card.height_action),
            }):spacing(8):width(size.fill),
        }):spacing(8)):padding(12):width(size.fill))
    end
    table.insert(rows, iced.masonry(placed):column_width(344):spacing(16))
    return iced.column(rows):spacing(12):width(size.fill)
end

return app
