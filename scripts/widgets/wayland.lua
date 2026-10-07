-- Basic Wayland overview (read-only): window count in the bar, every
-- toplevel in a popup listview. No actions exist — windows are never
-- focused, moved, or closed from here.
local app = {}

function app:view()
    local n = #wayland.toplevels
    if n == 0 then return ui.text("--") end
    return ui.text(n == 1 and "1 window" or (n .. " windows"))
end

function app:popup()
    -- Flat row model, one row per toplevel. Keys must be unique (the
    -- shell rejects duplicates); the protocol carries no window ids,
    -- so identical app/title pairs get an occurrence count to stay
    -- stable across renders.
    local rows = {}
    local seen = {}
    for _, tl in ipairs(wayland.toplevels) do
        local title = tl.title ~= "" and tl.title or tl.app_id
        local tag = tl.app_id .. ":" .. title
        seen[tag] = (seen[tag] or 0) + 1
        rows[#rows + 1] = {
            key = tag .. "#" .. seen[tag],
            label = tl.app_id .. " — " .. title,
        }
    end
    if #rows == 0 then return { text = "no windows", width = 340 } end
    local list = ui.listview(rows):id("wayland-toplevels"):key("key"):pitch(28)
        :delegate(function(item)
            return ui.row({ ui.text(item.label):width(300) })
        end)
        :onEntered({ x = { from = 200, to = 0 }, opacity = { from = 0, to = 1 }, duration = 250 })
        :onExit({ x = { to = -200 }, opacity = { to = 0 }, duration = 250 })
        :onDisplaced({ duration = 250 })
    return { ui = ui.card({ title = "Windows", body = list }), width = 340 }
end

return app
