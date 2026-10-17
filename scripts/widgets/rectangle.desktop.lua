-- Desktop apps own their size and appearance; Rust owns placement.
local app = {
    defaults = { width = 240, height = 160, radius = 12 },
}

function app:view(props)
    return iced.container(iced.space())
        :width(props.width):height(props.height)
        :radius(props.radius)
        :background(props.color or (self.hovered and theme.primary or theme.surface))
end

function app:actions(events)
    if events.type == "hold" or (events.type == "click" and events.button == "right") then
        self.reposition()
    end
end

return app
