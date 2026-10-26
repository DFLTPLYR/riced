-- Generates a Material You theme from a seed through the `theme_gen` service.
-- The service only builds the scheme; this app decides how to present it.
local app = {}

local SEEDS = { "#89b4fa", "#f38ba8", "#a6e3a1", "#fab387" }

function app:mount()
    self.theme_gen = riced.service("theme_gen")
    self.index = 1
    self:generate()
end

function app:generate()
    self.seed = SEEDS[self.index]
    self.pending = true
    self.theme_gen:command("generate", {
        seed = self.seed,
        variant = "tonalspot",
        darkmode = true,
    }, function(result)
        self.pending = false
        if result.ok then
            self.payload = result.data.payload
            self.error = nil
        else
            self.error = result.error.message
        end
    end)
end

function app:actions(event)
    if event.type == "action" and event.key == "regenerate" then
        self.index = self.index % #SEEDS + 1
        self:generate()
    end
end

function app:view()
    local colors = theme
    local dark = self.payload and self.payload.dark
    local surface = dark and dark.surface or colors.surface
    local on_surface = dark and dark.on_surface or colors.on_surface
    local primary = dark and dark.primary or colors.primary
    local status
    if self.error then
        status = self.error
    elseif self.pending then
        status = "Generating " .. (self.seed or "") .. "…"
    else
        status = "Seed " .. (self.seed or "")
    end
    return iced.layer({
        id = "theme_gen",
        layer = "top",
        anchors = { top = true, right = true },
        width = 300,
        height = 200,
        margin = { top = 272, right = 16 },
        content = iced.container(iced.column({
            iced.text("Theme generation"):color(on_surface),
            iced.text(status):color(on_surface),
            iced.text("Material primary"):color(primary),
            iced.container(iced.space()):width(size.fill):height(24):background(primary):radius(6),
            iced.button("Next seed", "regenerate"),
        }):spacing(12)):padding(16):width(size.fill):height(size.fill)
            :background(surface):radius(12),
    })
end

return app
