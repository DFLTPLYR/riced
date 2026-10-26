-- Example project-owned Settings page.
--
-- riced installs the bundled pages into the project's `settings/` directory on
-- first run and then adopts that directory as user-owned. Because this
-- directory is committed, the example starts from this single page and the
-- other pages keep using their bundled sources. Deleting this file falls back
-- to the bundled page; saving it reloads Settings while the window is open.
--
-- The host supplies the same `props` and validates the same actions as the
-- bundled page (`animation:fast`, `animation:medium`, `animation:slow`), so only
-- presentation changes here. Unknown action keys are ignored by the host.
local app = {}

function app:view(props)
    local choices = {}
    for _, speed in ipairs(props.speeds) do
        local selected = speed.id == props.selected
        local caption = speed.title .. " · " .. string.format("%.2f s", speed.duration_ms / 1000)
        if selected then caption = caption .. " · active" end
        table.insert(choices, iced.container(iced.column({
            iced.button(caption, "animation:" .. speed.id)
                :width(size.fill):padding(14)
                :background(selected and theme.primary or theme.surface)
                :color(selected and theme.on_primary or theme.on_surface),
            iced.text(speed.description):size(12):color(theme.on_surface),
        }):spacing(5)):width(size.fill))
    end
    return iced.column({
        iced.text("Transition speed"):size(16):color(theme.on_surface),
        iced.text("Project page: examples/settings/animation.lua. "
            .. "Edit and save it to reload this page without restarting the shell.")
            :size(12):wrap("word"):color(theme.on_surface),
        iced.column(choices):spacing(10):width(size.fill),
    }):spacing(12):width(size.fill)
end

return app
