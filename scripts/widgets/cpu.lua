-- Usage-only label (no icon).
local app = {}

function app:view()
    return ui.text(string.format("%.0f%%", sysinfo.cpu_usage))
end

return app
