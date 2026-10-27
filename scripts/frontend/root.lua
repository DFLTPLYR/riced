-- Root entry point: every surface is declared here.
--
-- Bars are Lua data (`bars.lua`) and widgets are `riced.widget` instances
-- embedded in `layers/bar.lua`; Rust only renders the declarations. Add or
-- remove a bar with `riced ipc add/remove`, which edits `bars.lua` directly.
local state = require("state")

local app = {}

local function default_widgets(edge)
    if edge == "top" or edge == "bottom" then
        return {
            start = { "workspaces" },
            center = { "clock" },
            ["end"] = { "systemtray", "notifycenter", "system" },
        }
    end
    return { start = { "workspaces" }, center = { "clock" }, ["end"] = { "system" } }
end

function app:mount(context)
    self.output = context.output.name
    self.panel = false
    self.bars = riced.file({
        type = "lua",
        path = "bars.lua",
        write_mode = "generated",
        defaults = {
            bars = {
                { edge = "top", thickness = 34, widgets = default_widgets("top") },
            },
        },
    })
    self.bars:watch(function(event)
        if event.ok then
            self.error = nil
        else
            self.error = event.error.message
        end
    end)
end

function app:save_bars()
    self.bars:save(function(saved)
        if saved.ok then
            self.error = nil
        else
            self.error = saved.error.message
        end
    end)
end

function app:add_bar(options)
    local bars = self.bars:get("bars") or {}
    for _, bar in ipairs(bars) do
        if bar.edge == options.anchor and (bar.output or "") == (options.output or "") then
            error("that edge already has a bar")
        end
    end
    table.insert(bars, {
        edge = options.anchor,
        output = options.output or "",
        thickness = options.thickness or 34,
        widgets = default_widgets(options.anchor),
    })
    self.bars:set("bars", bars)
    self:save_bars()
end

function app:remove_bar(options)
    local bars = self.bars:get("bars") or {}
    for index, bar in ipairs(bars) do
        if bar.edge == options.anchor and (bar.output or "") == (options.output or "") then
            table.remove(bars, index)
            self.bars:set("bars", bars)
            self:save_bars()
            return
        end
    end
    error("no matching bar")
end

function app:view(props)
    local layers = {}
    layers[#layers + 1] = riced.layer("layers/background.lua", {
        output = props.output,
        wallpaper = props.wallpaper,
    })
    local output = props.output and props.output.name or ""
    for _, bar in ipairs(self.bars:get("bars") or {}) do
        local scoped = bar.output or ""
        if scoped == "" or scoped == output then
            local edge = bar.edge or "top"
            layers[#layers + 1] = riced.layer("layers/bar.lua", {
                id = "bar-" .. edge,
                edge = edge,
                thickness = bar.thickness or 34,
                widgets = bar.widgets or {},
                output = props.output,
            })
        end
    end
    if self.panel then
        layers[#layers + 1] = riced.layer("layers/panel.lua", {
            bars = self.bars:get("bars") or {},
        })
    end
    return layers
end

function app:actions(event)
    local key = event.key
    if key == "settings" then
        return shell.open_settings()
    end
    if key == "panel" then
        self.panel = not self.panel
        return
    end
    if key == "panel:close" then
        self.panel = false
        return
    end
    local edge = key and key:match("^panel:add:(.+)$")
    if edge then
        pcall(self.add_bar, self, { anchor = edge, thickness = 34 })
        return
    end
    local remove = key and key:match("^panel:remove:(.+)$")
    if remove then
        pcall(self.remove_bar, self, { anchor = remove })
    end
end

return app
