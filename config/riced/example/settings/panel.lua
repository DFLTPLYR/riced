-- Bar panel page. Bars are plain data files under `data/bars/`; this page
-- lists them, edits their slots and widgets, and writes the same files
-- shell.lua reads. No host state and no hardcoded bar list.
local app = {}

local function bar_path(name)
    return "data/bars/" .. name
end

local function stem(name)
    return (name:gsub("%.lua$", ""))
end

local function load_bar(name)
    local ok, bar = pcall(riced.read, bar_path(name))
    if ok and type(bar) == "table" then
        return bar
    end
    return nil
end

local function save_bar(name, bar)
    riced.write(bar_path(name), bar)
end

local function widget_names()
    local out = {}
    for _, file in ipairs(riced.list("widgets")) do
        local name = stem(file)
        if name ~= "" and not name:match("%.desktop$") then
            out[#out + 1] = name
        end
    end
    return out
end

local function hint(text)
    return iced.text(text):size(12):wrap("word"):color(theme.on_surface)
end

local function heading(title)
    return iced.text(title):size(16):color(theme.on_surface)
end

local function button(label, action)
    return iced.button(label, action):padding(6)
end

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

function app:view(props)
    local rows = {
        hint("Bars are files in data/bars/. Editing here writes those files; shell.lua declares one layer per file."),
    }
    local names = riced.list("data/bars")
    if #names == 0 then
        table.insert(rows, hint("No bars yet. Add one below."))
    end
    local widgets = widget_names()
    for _, file in ipairs(names) do
        local bar = load_bar(file)
        if bar then
            local name = stem(file)
            table.insert(rows, heading(name .. " · " .. (bar.edge or "top")
                .. " · " .. tostring(bar.thickness or 34) .. "px"))
            table.insert(rows, iced.row({
                button("-", "settings:lua:thickness:" .. name .. ":-2"),
                button("+", "settings:lua:thickness:" .. name .. ":2"),
                button("Remove bar", "settings:lua:remove-bar:" .. name),
            }):spacing(6))
            for index, slot in ipairs(bar.slots or {}) do
                table.insert(rows, hint("Slot " .. index .. " · " .. (slot.align or "center")))
                local current = {}
                for position, widget in ipairs(slot.widgets or {}) do
                    current[#current + 1] = button(widget .. " ✕",
                        "settings:lua:remove-widget:" .. name .. ":" .. index .. ":" .. position)
                end
                table.insert(rows, iced.row(current):spacing(6))
                local available = {}
                for _, widget in ipairs(widgets) do
                    available[#available + 1] = button("+" .. widget,
                        "settings:lua:add-widget:" .. name .. ":" .. index .. ":" .. widget)
                end
                table.insert(rows, iced.row(available):spacing(6))
                table.insert(rows, button("Remove slot", "settings:lua:remove-slot:" .. name .. ":" .. index))
            end
            table.insert(rows, button("Add slot", "settings:lua:add-slot:" .. name))
            table.insert(rows, iced.separator())
        end
    end
    table.insert(rows, heading("Add bar"))
    table.insert(rows, iced.row({
        button("Top", "settings:lua:add-bar:top"),
        button("Bottom", "settings:lua:add-bar:bottom"),
        button("Left", "settings:lua:add-bar:left"),
        button("Right", "settings:lua:add-bar:right"),
    }):spacing(6))
    return iced.column(rows):spacing(12):width(size.fill)
end

function app:actions(event)
    local key = event.key or ""
    local edge = key:match("^settings:lua:add%-bar:(.+)$")
    if edge then
        if not pcall(riced.read, bar_path(edge .. ".lua")) then
            save_bar(edge .. ".lua", {
                edge = edge,
                thickness = 34,
                slots = default_slots(edge),
            })
        end
        return
    end
    local name = key:match("^settings:lua:remove%-bar:(.+)$")
    if name then
        riced.remove(bar_path(name .. ".lua"))
        return
    end
    name = key:match("^settings:lua:add%-slot:(.+)$")
    if name then
        local bar = load_bar(name .. ".lua")
        if bar then
            bar.slots = bar.slots or {}
            table.insert(bar.slots, { align = "center", widgets = {} })
            save_bar(name .. ".lua", bar)
        end
        return
    end
    local index
    name, index = key:match("^settings:lua:remove%-slot:([^:]+):(%d+)$")
    if name then
        local bar = load_bar(name .. ".lua")
        if bar then
            table.remove(bar.slots or {}, tonumber(index))
            save_bar(name .. ".lua", bar)
        end
        return
    end
    local widget
    name, index, widget = key:match("^settings:lua:add%-widget:([^:]+):(%d+):(.+)$")
    if name then
        local bar = load_bar(name .. ".lua")
        if bar and bar.slots and bar.slots[tonumber(index)] then
            local slot = bar.slots[tonumber(index)]
            slot.widgets = slot.widgets or {}
            table.insert(slot.widgets, widget)
            save_bar(name .. ".lua", bar)
        end
        return
    end
    local widget_index
    name, index, widget_index = key:match("^settings:lua:remove%-widget:([^:]+):(%d+):(%d+)$")
    if name then
        local bar = load_bar(name .. ".lua")
        if bar and bar.slots and bar.slots[tonumber(index)] then
            table.remove(bar.slots[tonumber(index)].widgets or {}, tonumber(widget_index))
            save_bar(name .. ".lua", bar)
        end
        return
    end
    local delta
    name, delta = key:match("^settings:lua:thickness:([^:]+):(-?%d+)$")
    if name then
        local bar = load_bar(name .. ".lua")
        if bar then
            bar.thickness = math.max(8, math.min(512, (bar.thickness or 34) + tonumber(delta)))
            save_bar(name .. ".lua", bar)
        end
    end
end

return app
