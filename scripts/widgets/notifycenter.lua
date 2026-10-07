local app = {}

function app:view()
    local count = #notifications
    if count == 0 then return ui.icon("bell") end
    return ui.row({ ui.icon("bell"), ui.text(tostring(count)) })
end

function app:popup()
    if #notifications == 0 then return { text = "No notifications", width = 300 } end
    local rows = ui.listview(notifications):id("notification-center"):key("id"):pitch(32)
        :delegate(function(item)
            local label = item.title ~= "" and item.title or item.body
            return ui.row({ ui.text(label):width(180), ui.button("{icon:x}", "dismiss:" .. item.id):width(40) })
        end)
        :onEntered({ x = { from = 200, to = 0 }, opacity = { from = 0, to = 1 }, duration = 250 })
        :onExit({ x = { to = -200 }, opacity = { to = 0 }, duration = 250 })
        :onDisplaced({ duration = 250 })
    return { ui = ui.card({ title = "Notifications", icon = "bell", body = rows }), width = 320 }
end

function app:on_action(key)
    local id = key:match("^dismiss:(%d+)$")
    if id then return { dismiss = tonumber(id) } end
end

return app
