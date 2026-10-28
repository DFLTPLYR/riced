-- Context-menu source selection. Rust owns config validation and reload.
local app = {}

local function hint(text)
    return iced.text(text):size(12):wrap("word"):color(theme.on_surface)
end

local function section(title, description, body)
    return iced.container(iced.column({
        iced.text(title):size(16):color(theme.on_surface),
        hint(description),
        body,
    }):spacing(16)):padding(18):width(size.fill)
end

local function editor(props, kind)
    return section(props[kind .. "_title"], props[kind .. "_description"], iced.column({
        iced.text("Source"):size(14):width(size.fill):color(theme.on_surface),
        iced.text_input("component.lua", props[kind]):on_input(kind):padding(6):width(size.fill),
        hint("Relative to components/; absolute paths also work. Changes reload live."),
    }):spacing(8):width(size.fill))
end

function app:view(props)
    return iced.column({
        editor(props, "surface"),
        editor(props, "item"),
    }):spacing(16):width(size.fill)
end

return app
