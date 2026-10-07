-- Basic Wayland overview (read-only): a workspace strip in the bar,
-- workspaces plus toplevels in the popup. This never manages windows:
-- no focus, move, or close dispatch exists here.
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
        cells[#cells + 1] = ui.text(label)
    end
    if #cells == 0 then return ui.text("--") end
    return ui.row(cells)
end

function app:popup()
    local sections = {}
    local workspaces = visible_workspaces()
    if #workspaces == 0 then
        sections[#sections + 1] = ui.text("no workspaces")
    end
    for _, ws in ipairs(workspaces) do
        sections[#sections + 1] = ui.text(ws.name .. " (" .. ws.monitor .. ") — " .. ws.windows)
        for _, tl in ipairs(wayland.toplevels) do
            if tl.workspace == ws.id then
                local title = tl.title ~= "" and tl.title or tl.class
                sections[#sections + 1] = ui.text("  " .. tl.class .. " — " .. title)
            end
        end
    end
    return {
        ui = ui.card({ title = "Wayland", body = ui.column(sections) }),
        width = 340,
    }
end

return app
