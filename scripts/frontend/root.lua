-- Root entry point: every surface is declared here.
--
-- Bars are plain data files under `data/bars/`. root.lua lists the directory
-- and declares one layer per file; Settings → Panel reads and writes the same
-- files. Nothing here hardcodes a bar or its widgets.
local state = require("state")

local app = {}

local function default_slots(edge)
    if edge == "top" or edge == "bottom" then
        return {
            { align = "start", widgets = { "workspaces" } },
            { align = "center", widgets = { "clock" } },
            { align = "end", widgets = { "systemtray", "notifycenter", "system" } },
        }
    end
    return {
        { align = "start", widgets = { "workspaces" } },
        { align = "center", widgets = { "clock" } },
        { align = "end", widgets = { "system" } },
    }
end

function app:mount(context)
    self.output = context.output.name
end

function app:add_bar(options)
    local path = "data/bars/" .. options.anchor .. ".lua"
    if pcall(riced.read, path) then
        error("that edge already has a bar")
    end
    riced.write(path, {
        edge = options.anchor,
        thickness = options.thickness or 34,
        slots = default_slots(options.anchor),
    })
end

function app:remove_bar(options)
    riced.remove("data/bars/" .. options.anchor .. ".lua")
end

function app:view(props)
    local layers = {
        riced.layer("layers/background.lua", {
            output = props.output,
            wallpaper = props.wallpaper,
        }),
    }
    for _, name in ipairs(riced.list("data/bars")) do
        local ok, bar = pcall(riced.read, "data/bars/" .. name)
        if ok and type(bar) == "table" then
            local stem = name:gsub("%.lua$", "")
            layers[#layers + 1] = riced.layer("layers/bar.lua", {
                id = "bar-" .. stem,
                edge = bar.edge or "top",
                thickness = bar.thickness or 34,
                slots = bar.slots or {},
                output = props.output,
            })
        end
    end
    return layers
end

function app:actions(event)
    if event.key == "settings" then
        return shell.open_settings()
    end
end

return app
