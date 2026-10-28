local app = {}

function app:view()
    local usage = system.gpu_usage
    return iced.text(usage == nil and "--" or string.format("%.0f%%", usage))
end

return app
