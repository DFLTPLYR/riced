-- Lists compositor toplevels from the `toplevels` extension service.
local app = {}

function app:mount()
    self.toplevels = riced.service("toplevels")
    local snapshot = self.toplevels:snapshot()
    self.rows = snapshot.data.toplevels or {}
    self.available = snapshot.available
    self.toplevels:subscribe(function(snapshot)
        self.rows = snapshot.data.toplevels or {}
        self.available = snapshot.available
    end)
end

function app:view()
    local colors = theme
    local lines = {}
    for _, row in ipairs(self.rows) do
        table.insert(lines, iced.text(row.app_id .. "  " .. row.title):color(colors.on_surface))
    end
    if #lines == 0 then
        table.insert(
            lines,
            iced.text(self.available and "No toplevels" or "Unavailable"):color(colors.on_surface)
        )
    end
    return iced.layer({
        id = "toplevels",
        layer = "top",
        anchors = { bottom = true, left = true },
        width = 320,
        height = 160,
        margin = { bottom = 16, left = 16 },
        content = iced.container(iced.column(lines):spacing(4)):padding(16)
            :width(size.fill):height(size.fill):background(colors.surface):radius(12),
    })
end

return app
