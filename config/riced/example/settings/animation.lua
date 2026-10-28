-- The native host supplies validated speed options and persists actions.
-- This module owns presentation only; it does not mutate config or do I/O.
local app = {}

function app:view(props)
    local choices = {}
    for _, speed in ipairs(props.speeds) do
        local selected = speed.id == props.selected
        local caption = speed.title .. " · " .. speed.duration_ms .. " ms"
        if selected then caption = caption .. " · Selected" end
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
        iced.text("Shorter durations feel snappier; longer durations make changes more gradual.")
            :size(12):wrap("word"):color(theme.on_surface),
        iced.column(choices):spacing(10):width(size.fill),
        iced.text("Some Lua listviews define their own durations and override this global setting.")
            :size(12):wrap("word"):color(theme.on_surface),
    }):spacing(12):width(size.fill)
end

return app
