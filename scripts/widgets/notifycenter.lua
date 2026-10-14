local app = {}

function app:view()
    local count = #notifications
    if count == 0 then return iced.icon("bell") end
    return iced.row({ iced.icon("bell"), iced.text(tostring(count)) })
end

function app:popup()
    if #notifications == 0 then return { text = "No notifications", width = 300 } end
    local rows = iced.listview(notifications):id("notification-center"):key("id"):pitch(32)
        :delegate(function(item)
            local label = item.title ~= "" and item.title or item.body
            return iced.row({ iced.text(label):width(180), iced.button("{icon:x}", "dismiss:" .. item.id):width(40) })
        end)
        :onEntered({ x = { from = 200, to = 0 }, opacity = { from = 0, to = 1 }, duration = 250 })
        :onExit({ x = { to = -200 }, opacity = { to = 0 }, duration = 250 })
        :onDisplaced({ duration = 250 })
    return { ui = iced.card({ title = "Notifications", icon = "bell", body = rows }), width = 320 }
end

function app:on_action(key)
    local id = key:match("^dismiss:(%d+)$")
    if id then return { dismiss = tonumber(id) } end
end

return app
