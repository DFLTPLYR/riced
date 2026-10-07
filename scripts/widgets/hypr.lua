-- Basic Wayland overview: a workspace strip in the bar, workspaces
-- plus toplevels in a popup listview. Clicking focuses that
-- workspace via hyprctl; windows are never moved or closed here.
local app = {}

-- Workspaces on the first known output (all of them while outputs are
-- unknown, e.g. off-compositor).
local function visible_workspaces()
    local output = wayland.outputs[1] and wayland.outputs[1].name or nil
    local list = {}
    for _, ws in ipairs(wayland.workspaces) do
        if not output or ws.monitor == output then
            list[#list + 1] = ws
        end
    end
    return list
end

function app:view()
    local cells = {}
    for pos, ws in ipairs(visible_workspaces()) do
        local label = wayland.active_workspace == ws.id and ("[" .. pos .. "]") or tostring(pos)
        cells[#cells + 1] = ui.button(label, "ws:" .. ws.id)
    end
    if #cells == 0 then return ui.text("--") end
    return ui.row(cells)
end

function app:popup()
    -- Flat row model: one header per workspace, one row per toplevel.
    -- Keys must be unique (the shell rejects duplicates); toplevels
    -- carry no compositor id, so identical class/title pairs get an
    -- occurrence count to stay stable across renders.
    local rows = {}
    local seen = {}
    for _, ws in ipairs(visible_workspaces()) do
        rows[#rows + 1] = {
            kind = "ws", key = "ws:" .. ws.id, id = ws.id,
            label = ws.name .. " (" .. ws.monitor .. ") — " .. ws.windows,
        }
        for _, tl in ipairs(wayland.toplevels) do
            if tl.workspace == ws.id then
                local title = tl.title ~= "" and tl.title or tl.class
                local tag = ws.id .. ":" .. tl.class .. ":" .. title
                seen[tag] = (seen[tag] or 0) + 1
                rows[#rows + 1] = {
                    kind = "tl", key = "tl:" .. tag .. "#" .. seen[tag], id = ws.id,
                    label = "  " .. tl.class .. " — " .. title,
                }
            end
        end
    end
    if #rows == 0 then return { text = "no workspaces", width = 340 } end
    local list = ui.listview(rows):id("wayland-overview"):key("key"):pitch(28)
        :delegate(function(item)
            return ui.row({ ui.button(item.label, "ws:" .. item.id):width(300) })
        end)
        :onEntered({ x = { from = 200, to = 0 }, opacity = { from = 0, to = 1 }, duration = 250 })
        :onExit({ x = { to = -200 }, opacity = { to = 0 }, duration = 250 })
        :onDisplaced({ duration = 250 })
    return { ui = ui.card({ title = "Wayland", body = list }), width = 340 }
end

function app:on_action(key)
    local id = key:match("^ws:(%d+)$")
    if id then os.execute("hyprctl dispatch workspace " .. id .. " >/dev/null 2>&1") end
end

return app
