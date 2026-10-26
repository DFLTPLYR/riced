-- One Lua instance owns the button layer and its modal popup on each output.
local app = {}

function app:mount()
    self.open = false
    self.count = 0
end

function app:actions(event)
    if event.type == "popup_closed" then
        self.open = false
    elseif event.type == "action" then
        if event.key == "toggle" then self.open = not self.open
        elseif event.key == "increment" then self.count = self.count + 1
        elseif event.key == "close" then self.open = false end
    end
end

function app:view(props)
    local popup
    if self.open then
        popup = {
            id = "counter",
            x = 16,
            y = (props.output.height or 600) - 240,
            width = 260,
            height = 160,
            content = iced.container(iced.column({
                iced.text("App-owned popup: " .. self.count),
                iced.button("Increment shared count", "increment"),
                iced.button("Close", "close"),
            }):spacing(12)):padding(16):width(size.fill):height(size.fill)
                :background(theme.surface):radius(12),
        }
    end
    return iced.layer({
        anchors = { bottom = true, left = true },
        width = 180,
        height = 48,
        margin = { bottom = 16, left = 16 },
        content = iced.button("Open menu (" .. self.count .. ")", "toggle")
            :width(size.fill):height(size.fill),
        popup = popup,
    })
end

return app
