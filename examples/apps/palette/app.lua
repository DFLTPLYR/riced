-- Uses the same Lua API for compiled-in and supervised palette services.
local app = {}

function app:mount()
    self.palette = riced.service("palette")
    local snapshot = self.palette:snapshot()
    self.available = snapshot.available
    self.colors = snapshot.data.colors
    self.palette:subscribe(function(snapshot)
        self.available = snapshot.available
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
    elseif event.type == "action" and event.key == "apply" and self.colors then
        return iced.palette(self.colors)
    end
end

function app:view()
    local applied = iced.palette()
    local colors = self.colors or applied
    return iced.layer({
        id = "palette",
        layer = "top",
        anchors = { top = true, right = true },
        width = 300,
        height = 240,
        margin = { top = 64, right = 16 },
        content = iced.container(iced.column({
            iced.text(self.error or "Palette service"):color(colors.on_surface),
            iced.text(self.available and "Service available" or "Unavailable — Generate retries")
                :color(colors.on_surface),
            iced.text("Material primary"):color(colors.primary),
            iced.text("Applied primary: " .. applied.primary):color(applied.primary),
            iced.button("Generate preview", "generate"),
            iced.button("Apply generated palette", "apply"),
        }):spacing(12)):padding(16):width(size.fill):height(size.fill)
            :background(colors.surface):radius(12),
    })
end

return app
