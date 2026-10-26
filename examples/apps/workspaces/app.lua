-- Shows compositor workspaces from the `workspaces` extension service.
local app = {}

function app:mount()
    self.workspaces = riced.service("workspaces")
    local snapshot = self.workspaces:snapshot()
    self.rows = snapshot.data.workspaces or {}
    self.workspaces:subscribe(function(snapshot)
        self.rows = snapshot.data.workspaces or {}
    end)
end

function app:view()
    local colors = theme
    local cells = {}
    for _, ws in ipairs(self.rows) do
        local label = ws.name ~= "" and ws.name or "?"
        local text = iced.text(label):color(ws.active and colors.primary or colors.on_surface)
        table.insert(cells, iced.container(text):padding(6))
    end
    if #cells == 0 then
        table.insert(cells, iced.text("No workspaces"):color(colors.on_surface))
    end
    return iced.layer({
        id = "workspaces",
        layer = "top",
        anchors = { top = true, left = true },
        width = 240,
        height = 40,
        margin = { top = 64, left = 16 },
        content = iced.container(iced.row(cells):spacing(6)):padding(8)
            :width(size.fill):height(size.fill):background(colors.surface):radius(12),
    })
end

return app
