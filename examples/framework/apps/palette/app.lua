-- Optional SDK demonstration: enable { name = "palette" } in config.lua.
local app = {}

function app:mount()
    self.palette = riced.service("palette")
    self.colors = self.palette:snapshot().data.colors
    self.palette:subscribe(function(snapshot)
        self.colors = snapshot.data.colors
    end)
    self:generate()
end

function app:generate()
    self.palette:command("generate", { seed = "#89b4fa", dark = true }, function(result)
        if result.ok then
            self.colors = result.data.colors
            self.error = false
        else
            self.error = result.error.message
        end
    end)
end

function app:actions(event)
    if event.type == "action" and event.key == "generate" then
        self:generate()
    end
end

function app:view()
    local colors = self.colors or theme
    return iced.layer({
        id = "palette",
        layer = "top",
        anchors = { top = true, right = true },
        width = 300,
        height = 160,
        margin = { top = 64, right = 16 },
        content = iced.container(iced.column({
            iced.text(self.error or "Compiled-in palette extension"):color(colors.on_surface),
            iced.text("Material primary"):color(colors.primary),
            iced.button("Generate palette", "generate"),
        }):spacing(12)):padding(16):width(size.fill):height(size.fill)
            :background(colors.surface):radius(12),
    })
end

return app
