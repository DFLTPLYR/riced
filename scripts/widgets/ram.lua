local app = {}

function app:view()
    return ui.text(string.format("%.0f%%", system.mem_usage))
end

return app
