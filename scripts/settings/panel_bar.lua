-- Bar-level Panel controls. Rust owns the bar snapshot, bounds and target identity.
local app = {}

local function hint(text)
    return iced.text(text):size(12):wrap("word"):color(theme.on_surface)
end

local function heading(title, description)
    return iced.column({
        iced.text(title):size(16):color(theme.on_surface),
        hint(description),
    }):spacing(5):width(size.fill)
end

local function slider_row(label, value, min, max, step, key)
    return iced.container(iced.column({
        iced.row({
            iced.text(label):size(14):width(size.fill):color(theme.on_surface),
            hint(string.format("%.0f – %.0f", min, max)),
        }):spacing(12),
        iced.slider(min, max, value):step(step):on_change(key):width(size.fill),
    }):spacing(10)):padding(12):width(size.fill)
end

local function spin_row(label, value, min, max, step, decimals, key)
    return iced.row({
        iced.text(label):size(14):width("fill:7"):color(theme.on_surface),
        iced.spin_box(value, min, max):step(step):decimals(decimals):on_change(key):width(120),
    }):spacing(8)
end

local function picker(entries)
    local cells = {}
    for _, entry in ipairs(entries) do
        table.insert(cells, iced.button(entry.label, entry.action):padding(entry.padding)
            :background(entry.selected and theme.primary or theme.surface)
            :color(entry.selected and theme.on_primary or theme.on_surface))
    end
    return iced.row(cells):spacing(8)
end

function app:view(props)
    if props.section == "layout" then
        local rows = {
            heading("Size & appearance", "Length is a percentage of the display edge; thickness is measured in pixels."),
            slider_row(string.format("Length %.0f%%", props.length), props.length, 1, 100, 1, "length"),
            slider_row(string.format("Thickness %.0fpx", props.thickness), props.thickness, 1, props.thickness_max, 1, "thickness"),
        }
        local opacity = { iced.text("Opacity"):width(size.fill) }
        for _, step in ipairs(props.opacity_steps) do
            table.insert(opacity, iced.button(string.format("%.0f%%", step.value * 100), step.action):padding(6)
                :background(step.selected and theme.primary or theme.surface)
                :color(step.selected and theme.on_primary or theme.on_surface))
        end
        table.insert(rows, iced.row(opacity):spacing(8))
        table.insert(rows, heading("Slots & widgets", "Slots divide the bar into regions. Select a slot to edit its alignment and widgets."))
        table.insert(rows, spin_row(string.format("Slots (%d)", props.slots), props.slots, 1, props.max_slots, 1, 0, "slots"))
        table.insert(rows, slider_row(string.format("Slot padding %.0fpx", props.slot_padding), props.slot_padding, 0, 32, 1, "slot_padding"))
        table.insert(rows, slider_row(string.format("Slot spacing %.0fpx", props.slot_spacing), props.slot_spacing, 0, 32, 1, "slot_spacing"))
        table.insert(rows, picker(props.slots_picker))
        table.insert(rows, picker(props.aligns))
        return iced.column(rows):spacing(16):width(size.fill)
    end
    local rows = {
        heading("Floating & corners", "Inset the bar from the display edge and soften its corners."),
        iced.checkbox("Floating bar", props.floating):on_toggle("floating"):width(size.fill),
        hint("Floating adds space around the bar; the reserved desktop area stays unchanged."),
    }
    if props.floating then
        for _, edge in ipairs(props.margins) do
            table.insert(rows, spin_row(edge.label, edge.value, 0, 256, 1, 0, edge.action))
        end
    else
        table.insert(rows, hint("Enable Floating to adjust margins."))
    end
    table.insert(rows, iced.separator():height(2))
    for start = 1, #props.corners, 2 do
        table.insert(rows, iced.row({
            slider_row(props.corners[start].label .. string.format(" %.0f", props.corners[start].value), props.corners[start].value, 0, 32, 1, props.corners[start].action),
            slider_row(props.corners[start + 1].label .. string.format(" %.0f", props.corners[start + 1].value), props.corners[start + 1].value, 0, 32, 1, props.corners[start + 1].action),
        }):spacing(8):width(size.fill))
    end
    table.insert(rows, heading("Remove this bar", "Remove the selected bar from the display and saved configuration."))
    table.insert(rows, iced.button(props.remove_label, "remove"):padding(8):color(theme.error))
    return iced.column(rows):spacing(16):width(size.fill)
end

return app
