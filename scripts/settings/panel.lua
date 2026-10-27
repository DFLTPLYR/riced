-- Bar panel page. It owns `bars.lua` through `riced.file`, so adding or
-- removing a bar here writes the same file root.lua reads to declare layers.
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

local function hint(text)
    return iced.text(text):size(12):wrap("word"):color(theme.on_surface)
end

function app:mount()
    self.bars = riced.file({
        type = "lua",
        path = "bars.lua",
        write_mode = "generated",
        defaults = { bars = {} },
    })
    self.bars:watch(function(event)
        if event.ok then
            self.error = nil
        else
            self.error = event.error.message
        end
    end)
end

function app:view(props)
    local rows = {
        hint("Bars live in bars.lua. root.lua declares one layer per entry."),
    }
    if self.error then
        table.insert(rows, hint(self.error))
    end
    if not self.bars:status().loaded then
        table.insert(rows, hint("Loading bars.lua…"))
        return iced.column(rows):spacing(12):width(size.fill)
    end
    local bars = self.bars:get("bars") or {}
    if #bars == 0 then
        table.insert(rows, hint("No bars yet. Add one below."))
    end
    for _, bar in ipairs(bars) do
        local edge = bar.edge or "top"
        local slots = #(bar.slots or {})
        table.insert(rows, iced.row({
            iced.text(edge .. " · " .. tostring(bar.thickness or 34) .. "px · "
                .. slots .. (slots == 1 and " slot" or " slots"))
                :width(size.fill):color(theme.on_surface),
            iced.button("Remove", "settings:lua:remove:" .. edge):padding(6),
        }):spacing(12):width(size.fill))
    end
    table.insert(rows, iced.row({
        iced.button("Add top", "settings:lua:add:top"):padding(6),
        iced.button("Add bottom", "settings:lua:add:bottom"):padding(6),
        iced.button("Add left", "settings:lua:add:left"):padding(6),
        iced.button("Add right", "settings:lua:add:right"):padding(6),
    }):spacing(8))
    return iced.column(rows):spacing(12):width(size.fill)
end

function app:actions(event)
    if not self.bars or not self.bars:status().loaded then
        return
    end
    local key = event.key or ""
    local edge = key:match("^settings:lua:add:(.+)$")
    if edge then
        local bars = self.bars:get("bars") or {}
        for _, bar in ipairs(bars) do
            if bar.edge == edge then return end
        end
        table.insert(bars, { edge = edge, thickness = 34, slots = default_slots(edge) })
        self.bars:set("bars", bars)
        self.bars:save(function() end)
        return
    end
    local remove = key:match("^settings:lua:remove:(.+)$")
    if remove then
        local bars = self.bars:get("bars") or {}
        for index, bar in ipairs(bars) do
            if bar.edge == remove then
                table.remove(bars, index)
                self.bars:set("bars", bars)
                self.bars:save(function() end)
                return
            end
        end
    end
end

return app
