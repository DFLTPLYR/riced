local app = { defaults = { menu_width = 190, fade_ms = 220 } }

function app:mount(context)
    -- Encode the connector bytes so even unusual output names cannot escape state/.
    local output = context.output.name:gsub(".", function(c)
        return string.format("%02x", string.byte(c))
    end)
    self.prefs = riced.file({
        type = "json",
        path = "state/" .. output .. ".json",
        create_parents = true,
        defaults = { wallpaper = "", color = "#181825" },
    })
    self.prefs:watch(function(event)
        if event.ok then self.error = nil
        else self.error = event.error.message end
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

function app:actions(event)
    if event.type == "action" then
        self.menu = nil
        if event.key == "pick-wallpaper" then
            self:pick_wallpaper()
        elseif event.key == "reload" and not self.prefs:status().busy then
            -- Explicitly discards local edits after a conflict.
            self.prefs:reload({ discard = true }, function(result)
                if result.ok then self.error = nil
                else self.error = result.error.message end
            end)
        end
        return
    end
    if event.type == "press" and event.button == "right" then
        self.menu = { x = event.x, y = event.y }
    elseif event.type == "press" and event.button == "left" then
        self.menu = nil
        if self.fade then self:cancel(self.fade); self.fade = nil end
        self.selection = { start_x = event.x, start_y = event.y,
            x = event.x, y = event.y, width = 0, height = 0 }
        self.selecting = true
    elseif event.type == "move" and self.selecting and self.selection then
        local s = self.selection
        s.x, s.y = math.min(s.start_x, event.x), math.min(s.start_y, event.y)
        s.width, s.height = math.abs(event.x - s.start_x), math.abs(event.y - s.start_y)
    elseif (event.type == "release" and event.button == "left") or event.type == "exit" then
        if not self.selecting then return end
        self.selecting = false
        self.fade = self:after(self.props.fade_ms, function()
            self.selection, self.fade = nil, nil
        end)
    end
end

function app:view(props)
    local output = props.output
    local layers = {
        iced.container(iced.space()):width(size.fill):height(size.fill)
            :background(self.prefs:get("color")),
    }
    local wallpaper = self.prefs:get("wallpaper")
    if wallpaper ~= "" then
        -- iced.image currently uses native contain fitting, not wallpaper cover.
        layers[#layers + 1] = iced.image(wallpaper):width(size.fill):height(size.fill)
    end
    layers[#layers + 1] = iced.container(iced.column({
        iced.text("Lua background · " .. output.name):size(24),
        iced.text("Right-click for wallpaper options · drag to select"),
        iced.button(self.picking and "Picker open…" or "Choose wallpaper", "pick-wallpaper"),
        iced.text(self.error or "Preferences reload automatically when edited")
            :color(self.error and theme.error or theme.on_surface),
    }):spacing(10)):padding(24):anchor("top_left")

    local s = self.selection
    if s and s.width > 0 and s.height > 0 then
        layers[#layers + 1] = iced.container(iced.space())
            :width(s.width):height(s.height)
            :background("#89b4fa22"):border("#89b4fa"):border_width(1)
            :anchor("top_left"):margin({ left = s.x, top = s.y })
            :animate({ id = "selection", opacity = iced.animations.opacity({
                to = self.selecting and 1 or 0, duration = props.fade_ms,
            }) })
    end
    if self.menu then
        -- Reserve enough room for three rows; constrain to small outputs too.
        local width = math.min(props.menu_width, output.width)
        local height = math.min(150, output.height)
        local x = math.max(0, math.min(self.menu.x, output.width - width))
        local y = math.max(0, math.min(self.menu.y, output.height - height))
        layers[#layers + 1] = riced.component("menu.lua", { width = width, height = height })
            :anchor("top_left"):margin({ left = x, top = y })
    end
    return iced.layer({
        id = "background",
        layer = "background",
        anchors = { top = true, right = true, bottom = true, left = true },
        exclusive_zone = -1,
        keyboard = "none",
        input = "all",
        pointer_events = true,
        content = iced.stack(layers):width(size.fill):height(size.fill),
    })
end

return app
