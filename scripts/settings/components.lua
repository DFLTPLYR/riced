-- Per-component defaults editor. Rust parses, validates, and persists values.
local app = {}

local function hint(text)
    return iced.text(text):size(12):wrap("word"):color(theme.on_surface)
end

function app:view(props)
    local rows = {
        hint("Explicit Lua setters override these values individually. Reset removes the saved default."),
        iced.row({
            iced.pick_list(props.components, props.component):on_select("component"):placeholder("Component"):width(size.fill),
            iced.pick_list(props.properties, props.property):on_select("property"):placeholder("Property"):width(size.fill),
        }):spacing(12):width(size.fill),
        iced.text_input(props.placeholder, props.draft):on_input("draft"):on_submit("submit"):padding(8):width(size.fill),
        iced.row({
            iced.button("Apply default", "submit"):padding(8),
            iced.button("Reset property", "reset"):padding(8),
        }):spacing(12),
        hint("Numbers and booleans are unquoted. Strings (including colors, sizes and action keys) need double quotes. Arrays and tables use { ... }. Empty, `nil`, or `none` clears the property."),
        iced.column({
            iced.text("Saved defaults"):size(16):color(theme.on_surface),
            hint("Persisted to config.lua under component_defaults.<component>."),
        }):spacing(5):width(size.fill),
    }
    if #props.saved == 0 then
        table.insert(rows, hint("No component defaults saved yet."))
    else
        for _, line in ipairs(props.saved) do
            table.insert(rows, iced.text(line):size(13):color(theme.on_surface))
        end
    end
    if props.error then
        table.insert(rows, iced.text(props.error):size(13):wrap("word"):color(theme.error))
    end
    return iced.column(rows):spacing(12):width(size.fill)
end

return app
