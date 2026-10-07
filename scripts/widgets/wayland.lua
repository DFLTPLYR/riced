-- Basic Wayland overview (read-only): workspace strip in the bar,
-- workspaces plus toplevels in a popup listview. No actions exist —
-- windows are never focused, moved, or closed from here.
local app = {}

-- Workspaces on this bar's output (all of them while the bar's
-- output is unknown, e.g. off-compositor). Workspaces whose monitor
-- is unresolvable (compositor never assigned their group to an
-- output) show on every bar rather than vanishing.
local function visible_workspaces()
    local output = bar.output
    local list = {}
    for _, ws in ipairs(wayland.workspaces) do
        if ws.monitor == "" then
            list[#list + 1] = ws
        elseif not output then
            list[#list + 1] = ws
        else
            for mon in (ws.monitor .. ","):gmatch("([^,]*),") do
                if mon == output then
                    list[#list + 1] = ws
                    break
                end
            end
        end
    end
    return list
end

function app:view()
    -- Horizontal strip, one cell per workspace; names are not unique
    -- across groups, so identical labels get an occurrence count to
    -- keep keys stable across renders.
    local cells = {}
    local seen = {}
    for _, ws in ipairs(visible_workspaces()) do
        local label = ws.active and ("[" .. ws.name .. "]") or ws.name
        local tag = ws.monitor .. ":" .. ws.name
        seen[tag] = (seen[tag] or 0) + 1
        cells[#cells + 1] = { key = "ws:" .. tag .. "#" .. seen[tag], label = label }
    end
    if #cells == 0 then return ui.text("--") end
    return ui.listview(cells):id("wayland-strip"):key("key"):axis("horizontal")
        :delegate(function(item)
            return ui.text(item.label)
        end)
        :onEntered({ opacity = { from = 0, to = 1 }, duration = 250 })
        :onExit({ opacity = { to = 0 }, duration = 250 })
        :onDisplaced({ duration = 250 })
end

function app:popup()
    -- Flat row model: one row per workspace, then one per toplevel.
    -- Keys must be unique (the shell rejects duplicates); neither
    -- carries a stable id, so identical labels get an occurrence
    -- count to stay stable across renders.
    local rows = {}
    local seen = {}
    local function push(kind, tag, label)
        seen[tag] = (seen[tag] or 0) + 1
        rows[#rows + 1] = { key = kind .. ":" .. tag .. "#" .. seen[tag], label = label }
    end
    for _, ws in ipairs(wayland.workspaces) do
        local head = ws.name
        if ws.monitor ~= "" then head = head .. " (" .. ws.monitor .. ")" end
        if ws.active then head = "[" .. head .. "]" end
        push("ws", ws.monitor .. ":" .. ws.name, head)
    end
    for _, tl in ipairs(wayland.toplevels) do
        local title = tl.title ~= "" and tl.title or tl.app_id
        push("tl", tl.app_id .. ":" .. title, "  " .. tl.app_id .. " — " .. title)
    end
    if #rows == 0 then return { text = "no windows", width = 340 } end
    local list = ui.listview(rows):id("wayland-overview"):key("key"):pitch(28)
        :delegate(function(item)
            return ui.row({ ui.text(item.label):width(300) })
        end)
        :onEntered({ x = { from = 200, to = 0 }, opacity = { from = 0, to = 1 }, duration = 250 })
        :onExit({ x = { to = -200 }, opacity = { to = 0 }, duration = 250 })
        :onDisplaced({ duration = 250 })
    return { ui = ui.card({ title = "Wayland", body = list }), width = 340 }
end

return app
