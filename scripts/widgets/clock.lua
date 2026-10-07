-- Module widget: return an app table; the shell calls app:view().
local app = {}

function app:view()
    return ui.row({ ui.icon("clock"), ui.text(os.date("%H:%M")) })
end

function app:popup()
    return { ui = ui.card({ title = "Clock", icon = "clock", body = os.date("%H:%M") }), width = 300 }
end

return app
