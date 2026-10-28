-- One entry point per output. This app owns its whole layer set: the wallpaper
-- surface plus one bar layer per entry in its own managed document. `view` just
-- declares them; the host creates one native surface per declared layer.
--
-- Adding a bar is app state, not a host command: `add-top` appends to
-- `bars.lua` and the next view declares the extra layer.
local app = { defaults = { menu_width = 190, fade_ms = 220, thickness = 32 } }

function app:mount(context)
    self.output = context.output.name
    -- Encode the connector bytes so even unusual output names cannot escape state/.
    local encoded = context.output.name:gsub(".", function(c)
        return string.format("%02x", string.byte(c))
    end)
    self.prefs = riced.file({
        type = "json",
        path = "state/" .. encoded .. ".json",
        create_parents = true,
        defaults = { wallpaper = "", color = "#181825" },
    })
    self.prefs:watch(function(event)
        if event.ok then self.error = nil
        else self.error = event.error.message end
    end)
    -- App-owned layer state. `bars.lua` is data, not app code, so editing it
    -- never reloads the app.
    self.bars = riced.file({
        type = "lua",
        path = "bars.lua",
        write_mode = "generated",
        defaults = { bars = {} },
    })
    self.bars:watch(function(event)
        if event.ok then self.error = nil
        else self.error = event.error.message end
    end)
    self.clock = os.date("%H:%M")
    self:every(1000, function()
        self.clock = os.date("%H:%M")
    end)
end

function app:pick_wallpaper()
    if self.picking or not self.prefs:status().loaded then return end
    self.picking = true
    self:pick_file({
        title = "Choose wallpaper",
        extensions = { "png", "jpg", "jpeg", "webp", "bmp" },
    }, function(result)
        self.picking = false
        if not result.ok then
            self.error = result.error.message
            return
        end
        if result.cancelled then return end
        local path = result.path
        local status = self.prefs:status()
        if status.busy then self.error = "Preferences are busy; choose the image again"; return end
        self.prefs:set("wallpaper", path)
        self.prefs:save(function(saved)
            if saved.ok then self.error = nil
            else self.error = saved.error.message end
        end)
    end)
end

-- Screen edge nearest the given point, in output-local coordinates. Policy
-- lives here in Lua; nothing asks the host to create a layer.
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

function app:add_bar(edge)
    local bars = self.bars:get("bars") or {}
    table.insert(bars, { edge = edge, thickness = self.props.thickness })
    self.bars:set("bars", bars)
    self.bars:save(function(saved)
        if saved.ok then self.error = nil
        else self.error = saved.error.message end
    end)
end

function app:remove_bar()
    local bars = self.bars:get("bars") or {}
    if #bars == 0 then return end
    table.remove(bars)
    self.bars:set("bars", bars)
    self.bars:save(function(saved)
        if saved.ok then self.error = nil
        else self.error = saved.error.message end
    end)
end

function app:actions(event)
    if event.type == "action" then
        local menu = self.menu
        self.menu = nil
        if event.key == "pick-wallpaper" then
            self:pick_wallpaper()
        elseif event.key == "reload" and not self.prefs:status().busy then
            -- Explicitly discards local edits after a conflict.
            self.prefs:reload({ discard = true }, function(result)
                if result.ok then self.error = nil
                else self.error = result.error.message end
            end)
        elseif event.key == "add-top" then
            self:add_bar(self:nearest_edge(menu and menu.x or 0, menu and menu.y or 0))
        elseif event.key == "remove-bar" then
            self:remove_bar()
        elseif event.key == "open-settings" then
            return shell.open_settings()
        end
        return
    end
    if event.layer and event.layer ~= "background" then return end
    local output = self.props.output
    if event.type == "press" and event.button == "right" then
        if event.source_output and event.source_output ~= self.output then return end
        self.menu = { x = event.x, y = event.y }
    elseif event.type == "press" and event.button == "left" then
        self.menu = nil
        self.selection_id = (self.selection_id or 0) + 1
        local x, y = event.x + output.x, event.y + output.y
        self.selection = { id = self.selection_id, start_x = x, start_y = y,
            x = x, y = y, width = 0, height = 0 }
        self.selecting = true
    elseif self.selecting and self.selection then
        local s = self.selection
        local released = event.type == "release" and event.button == "left"
        if (event.type == "move" or released) and event.x and event.y then
            local x, y = event.x + output.x, event.y + output.y
            s.x, s.y = math.min(s.start_x, x), math.min(s.start_y, y)
            s.width, s.height = math.abs(x - s.start_x), math.abs(y - s.start_y)
        end
        if released or event.type == "cancel" then self.selecting = false end
    end
end

function app:view(props)
    local layers = {
        riced.layer("layers/wallpaper.lua", {
            output = props.output,
            color = self.prefs:get("color"),
            wallpaper = self.prefs:get("wallpaper"),
            selecting = self.selecting,
            selection = self.selection,
            menu = self.menu,
            menu_width = props.menu_width,
            fade_ms = props.fade_ms,
            picking = self.picking,
            error = self.error,
        }),
    }
    for index, bar in ipairs(self.bars:get("bars") or {}) do
        layers[#layers + 1] = riced.layer("layers/bar.lua", {
            id = "bar-" .. index,
            output = self.output,
            edge = bar.edge,
            thickness = bar.thickness,
            clock = self.clock,
        })
    end
    return layers
end

return app
