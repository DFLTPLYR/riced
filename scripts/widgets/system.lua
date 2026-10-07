-- Native systemctl dispatch; task-based effects arrive in M4.
local app = {}
local allowed = { suspend = true, hibernate = true, reboot = true, poweroff = true }

function app:view()
    return ui.icon("power")
end

function app:popup()
    return {
        ui = ui.card({ title = "Session", icon = "power", body = ui.menu({
            id = "session-actions",
            items = {
                { label = "Suspend", action = "suspend" },
                { label = "Hibernate", action = "hibernate" },
                { label = "Reboot", action = "reboot" },
                { label = "Power off", action = "poweroff" },
            },
        }) }),
        width = 220,
    }
end

function app:on_action(key)
    if allowed[key] then os.execute("systemctl " .. key) end
end

return app
