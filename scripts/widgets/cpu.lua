-- Usage-only label (no icon).
local app = {}

function app:view()
    return ui.text(string.format("%.0f%%", system.cpu_usage))
end

return app
