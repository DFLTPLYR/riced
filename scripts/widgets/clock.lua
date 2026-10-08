-- Module widget: return an app table; the shell calls app:view().
-- `defaults` declares this widget's own interval, size, and custom
-- properties (edited per placement in Settings; `self.props` carries
-- the resolved values). `property_schema` optionally describes each
-- property for nicer editor controls.
local app = {
    defaults = {
        interval = 1.0,
        size = 13.0,
        props = {
            format = "%H:%M",
            show_seconds = false,
        },
    },
    property_schema = {
        format = {
            label = "Time format",
            type = "string",
            description = "Uses Lua's os.date format.",
        },
        show_seconds = {
            label = "Show seconds",
            type = "boolean",
        },
    },
}

local function stamp(props)
    if props.show_seconds then
        return os.date("%H:%M:%S")
    end
    return os.date(props.format or "%H:%M")
end

function app:view()
    return ui.row({ ui.icon("clock"), ui.text(stamp(self.props)) })
end

function app:popup()
    return { ui = ui.card({ title = "Clock", icon = "clock", body = stamp(self.props) }), width = 300 }
end

return app
