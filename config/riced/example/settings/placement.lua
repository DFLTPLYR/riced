-- A flat, typed snapshot; action keys are opaque and validated by the host.
local app = {}

local function hint(text)
    return iced.text(text):size(12):wrap("word"):color(theme.on_surface)
end

local function control(field)
    if field.read_only then return hint("Invalid numeric bounds in widget schema") end
    if field.kind == "boolean" then
        return iced.checkbox(field.label, field.value == true):on_toggle(field.action):width(size.fill)
    elseif field.kind == "number" then
        local value = type(field.value) == "number" and field.value or 0
        value = math.max(field.min, math.min(field.max, value))
        return iced.spin_box(value, field.min, field.max):step(field.step)
            :decimals(field.decimals):on_change(field.action):width(size.fill)
    elseif #field.choices > 0 then
        local choices = {}
        for _, choice in ipairs(field.choices) do
            local selected = choice.value == field.value
            table.insert(choices, iced.button(choice.value, choice.action):padding(6)
                :background(selected and theme.primary or theme.surface)
                :color(selected and theme.on_primary or theme.on_surface))
        end
        return iced.row(choices):spacing(8)
    end
    return iced.text_input("", type(field.value) == "string" and field.value or "")
        :on_input(field.action):padding(6):size(12):width(size.fill)
end

function app:view(props)
    if type(props.message) == "string" then return hint(props.message) end
    local rows = {
        iced.text(props.title):size(16):color(theme.on_surface),
        hint("Overrides apply to this instance only. Reset returns to the widget default."),
    }
    for _, field in ipairs(props.fields) do
        local header = { iced.text(field.label):size(14):width(size.fill):color(theme.on_surface) }
        if type(field.reset) == "string" then
            table.insert(header, iced.button("Reset", field.reset):padding(6))
        end
        local body = { iced.row(header):spacing(12), control(field) }
        if type(field.subtitle) == "string" then table.insert(body, hint(field.subtitle)) end
        table.insert(rows, iced.container(iced.column(body):spacing(8)):padding(12):width(size.fill))
    end
    if not props.has_properties then
        table.insert(rows, hint("This widget declares no custom properties — interval and size above are the knobs."))
    end
    return iced.column(rows):spacing(10):width(size.fill)
end

return app
