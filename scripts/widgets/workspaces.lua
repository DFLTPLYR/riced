-- Workspace strip on this bar's output. Metadata comes from native
-- ext-workspace; clicks focus the workspace via hyprctl.
local app = {}

local function visible_workspaces()
    local output = bar.output
    local list = {}
    for _, ws in ipairs(wayland.workspaces) do
        if not output then
            list[#list + 1] = ws
        else
            for monitor in ws.monitor:gmatch("[^,]+") do
                if monitor == output then
                    list[#list + 1] = ws
                    break
                end
            end
        end
    end
    return list
end

function app:view()
    -- Number cells locally; actions keep the actual workspace name.
    local cells = {}
    local seen = {}
    for _, ws in ipairs(visible_workspaces()) do
        local tag = ws.monitor .. ":" .. ws.name
        seen[tag] = (seen[tag] or 0) + 1
        cells[#cells + 1] = {
            key = "ws:" .. tag .. "#" .. seen[tag],
            label = #cells + 1,
            id = ws.name,
        }
    end
    if #cells == 0 then return ui.text("--") end
    return ui.listview(cells)
        :id("wayland-strip")
        :key("key")
        :axis("horizontal")
        :delegate(function(item)
            return ui.button(item.label or "?", item.id or "")
        end)
        :onEntered({ opacity = { from = 0, to = 1 }, duration = 250 })
        :onExit({ opacity = { to = 0 }, duration = 250 })
        :onDisplaced({ duration = 250 })
end

function app:on_action(id)
    if id then
        os.execute("hyprctl dispatch 'hl.dsp.focus({workspace = " .. id .. "})' >/dev/null 2>&1")
    end
end

return app
