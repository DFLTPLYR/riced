local app = {}

function app:view()
    local usage = gfxinfo.usage
    return ui.text(usage == nil and "--" or string.format("%.0f%%", usage))
end

return app
