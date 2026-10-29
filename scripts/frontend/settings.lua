local settings = {}

function settings:view()
    return iced.window({
        id = "settings",
        title = "Riced Settings",
        width = 1100,
        height = 760,
        content = riced.plugin("settings"),
    })
end

return settings
