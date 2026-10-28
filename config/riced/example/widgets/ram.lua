local app = {}

function app:view()
    return iced.text(string.format("%.0f%%", system.mem_usage))
end

return app
