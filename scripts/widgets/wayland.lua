-- Basic Wayland overview (read-only): window count in the bar,
-- workspaces plus toplevels in a popup listview. No actions exist —
-- windows are never focused, moved, or closed from here.
local app = {}

function app:view()
    local n = #wayland.toplevels
    if n == 0 then return ui.text("--") end
    return ui.text(n == 1 and "1 window" or (n .. " windows"))
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
