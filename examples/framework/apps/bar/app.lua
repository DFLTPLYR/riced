local app = { defaults = { height = 32 }, clicks = 0 }

function app:mount(context)
    self.output = context.output.name
end

function app:actions(event)
    if event.key == "increment" then
        self.clicks = self.clicks + 1
    end
end

function app:view(props)
    return iced.layer({
        id = "main",
        layer = "top",
        anchors = { top = true, left = true, right = true },
        height = props.height,
        exclusive_zone = props.height,
        keyboard = "none",
        input = "all",
        content = iced.container(
            iced.row({
                riced.component("status.lua", { output = self.output }),
                iced.space():width(size.fill),
                iced.button("Clicks: " .. self.clicks, "increment"),
                iced.text(os.date("%H:%M:%S")),
            }):spacing(12):height(size.fill):align_y("center")
        ):padding({ left = 12, right = 12 })
            :background(theme.surface):width(size.fill):height(size.fill),
    })
end

return app
