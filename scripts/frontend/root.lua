-- Root entry point: commands, shared state, and optional Lua surfaces.
--
-- Native hosts still own their surfaces by default (`bar_host = true`,
-- `background.disabled = false`). Turn a host off in config.lua to let this
-- file compose the matching module from `layers/` instead. While a native
-- host is on, its Lua layer is skipped so the same edge is never drawn twice.
local state = require("state")

local app = { menus = {}, recipes = {} }

function app:mount(context)
    self.clock = os.date("%H:%M")
    self:every(1000, function()
        self.clock = os.date("%H:%M")
    end)
end

-- Screen edge nearest a point in output-local coordinates. Used by the
-- background's "Add bar" action so the new bar lands where it was clicked.
function app:nearest_edge(x, y)
    local output = self.props.output or {}
    local width, height = output.width or 0, output.height or 0
    local best, best_distance = "top", y
    for _, candidate in ipairs({
        { "bottom", height - y },
        { "left", x },
        { "right", width - x },
    }) do
        if candidate[2] < best_distance then
            best, best_distance = candidate[1], candidate[2]
        end
    end
    return best
end

function app:view(props)
    local layers = {}
    self.recipes = {}
    if props.background_disabled then
        layers[#layers + 1] = riced.layer("layers/background.lua", {
            output = props.output,
            wallpaper = props.wallpaper,
            menu = self.menus.background,
        })
    end
    if not props.bar_host then
        for _, recipe in ipairs(props.bars or {}) do
            local edge = recipe.anchor or "top"
            local id = "bar-" .. edge
            self.recipes[id] = recipe
            self.recipes[edge] = recipe
            layers[#layers + 1] = riced.layer("layers/bar.lua", {
                id = id,
                output = props.output,
                edge = edge,
                thickness = math.floor((recipe.thickness or 34) + 0.5),
                clock = self.clock,
                menu = self.menus[id],
            })
        end
    end
    if #layers == 0 then
        return nil
    end
    return layers
end

function app:actions(event)
    if event.type == "press" then
        if event.button == "right" then
            self.menus = {}
            if event.layer ~= nil then
                self.menus[event.layer] = { x = event.x, y = event.y }
            end
        else
            self.menus = {}
        end
        return
    end
    if event.key == nil then
        return
    end
    local key = event.key
    if key == "settings" then
        self.menus = {}
        return shell.open_settings()
    end
    if key == "add-bar" then
        local menu = self.menus.background or { x = 0, y = 0 }
        self.menus = {}
        return shell.add_bar({
            anchor = self:nearest_edge(menu.x, menu.y),
            output = self.props.output.name,
            thickness = 34,
        })
    end
    local selected = key:match("^remove:(.+)$")
    if selected then
        self.menus = {}
        local recipe = self.recipes[selected]
        return shell.remove_bar({
            anchor = recipe and recipe.anchor or selected,
            output = recipe and recipe.output or self.props.output.name,
        })
    end
end

return app
