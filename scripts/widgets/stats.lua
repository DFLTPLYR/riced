-- State belongs to this app instance, not the shared Lua globals.
local app = { details = false }

function app:view()
    return iced.row({
        iced.icon("cpu"), iced.text(string.format("%.0f%%", system.cpu_usage)),
        iced.icon("memory-stick"), iced.text(string.format("%.0f%%", system.mem_usage)),
    })
end

function app:popup()
    local lines = {
        iced.row({ iced.icon("cpu"), iced.text(string.format("CPU  %.1f%%", system.cpu_usage)) }),
        iced.row({ iced.icon("memory-stick"), iced.text(string.format("Mem  %.1f%%", system.mem_usage)) }),
    }
    if self.details then
        lines[#lines + 1] = iced.row({ iced.icon("cpu"), iced.text("Cores " .. system.cpu_count) })
    end
    return {
        ui = iced.card({ title = "System stats", body = iced.column(lines) }),
        width = 300,
        items = {{ label = self.details and "{icon:arrow-up} Less" or "{icon:arrow-down} More", action = "toggle" }},
    }
end

function app:actions(events)
    if events.type == "click" and events.button == "left" then
        self:popup()
        return
    end
    if events.type ~= "action" then return end
    local key = events.key
    if key == "toggle" then self.details = not self.details end
end

return app
