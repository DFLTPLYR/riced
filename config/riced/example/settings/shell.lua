-- Settings shell: the page catalog, navigation, and page host live here.
-- Rust supplies the validated page id and the realized page through the
-- `settings-page` native slot. Page modules own their own content.
local app = {}

local pages = {
    { id = "info", title = "Info", description = "Overview of this Settings panel.", content = false },
    { id = "desktop", title = "Desktop", description = "Click inside a widget preview, then click the desktop to place it. Hold and drag a desktop widget; release to drop. Right-click or Escape cancels." },
    { id = "components", title = "Components", description = "Per-component defaults for every Lua component. Explicit setters take precedence." },
    { id = "context_menu", title = "Context Menu", description = "Customize the menu opened by right-clicking the wallpaper." },
    { id = "panel", title = "Panel", description = "Bars declared in bars.lua. Add or remove a bar." },
    { id = "wallpaper", title = "Wallpaper", description = "Arrange images across your displays and adjust their placement.", hint = "Wallpaper edits stay in this preview until you press Apply." },
    { id = "theme", title = "Theme", description = "Choose a palette and preview its colors in light or dark mode." },
    { id = "animation", title = "Animation", description = "Set the pace of animated color and property changes." },
    { id = "about", title = "About", description = "", content = false },
}

function app:view(props)
    local nav = {
        iced.text("RICED"):size(20):color(theme.on_surface),
        iced.text("Make it yours"):size(12):color(theme.on_surface),
        iced.separator(),
    }
    local selected
    for _, page in ipairs(pages) do
        if page.id == props.selected then selected = page end
        table.insert(nav, iced.button(page.title, "settings:page:" .. page.id)
            :width(size.fill):padding(12)
            :background(page.id == props.selected and theme.primary or theme.surface)
            :color(page.id == props.selected and theme.on_primary or theme.on_surface))
    end

    local body
    if selected and selected.content == false then
        body = iced.space():width(size.fill):height(size.fill)
    else
        body = iced.column({
            iced.column({
                iced.text(selected and selected.title or ""):size(26):color(theme.on_surface),
                iced.text(selected and selected.description or ""):size(12):wrap("word"):color(theme.on_surface),
            }):spacing(6):width(size.fill),
            riced.plugin("settings-page"),
            iced.text(selected and selected.hint or "Changes apply live and are saved automatically.")
                :size(12):wrap("word"):color(theme.on_surface),
        }):spacing(20):width(size.fill)
    end

    return iced.container(iced.row({
        iced.scrollable(iced.column(nav):spacing(10):width(size.fill)):width(168):height(size.fill),
        iced.scrollable(iced.column({ body }):spacing(8)):width(size.fill):height(size.fill),
    }):spacing(24)):padding(24):width(size.fill):height(size.fill)
        :background(theme.surface)
end

return app
