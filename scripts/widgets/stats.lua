-- State belongs to this app instance, not the shared Lua globals.
local app = { details = false }

function app:view()
    return ui.row({
        ui.icon("cpu"), ui.text(string.format("%.0f%%", sysinfo.cpu_usage)),
        ui.icon("memory-stick"), ui.text(string.format("%.0f%%", sysinfo.mem_usage)),
    })
end

function app:popup()
    local lines = {
        ui.row({ ui.icon("cpu"), ui.text(string.format("CPU  %.1f%%", sysinfo.cpu_usage)) }),
        ui.row({ ui.icon("memory-stick"), ui.text(string.format("Mem  %.1f%%", sysinfo.mem_usage)) }),
    }
    if self.details then
        lines[#lines + 1] = ui.row({ ui.icon("cpu"), ui.text("Cores " .. sysinfo.cpu_count) })
    end
    return {
        ui = ui.card({ title = "System stats", body = ui.column(lines) }),
        width = 300,
        items = {{ label = self.details and "{icon:arrow-up} Less" or "{icon:arrow-down} More", action = "toggle" }},
    }
end

function app:on_action(key)
    if key == "toggle" then self.details = not self.details end
end

return app
