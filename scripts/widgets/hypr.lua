-- Workspace strip backed by `wayland.workspaces` (no shell-outs on
-- the render path; only `on_action` still dispatches via hyprctl).
local app = { workspace_ids = {} }

function app:view()
    local output = wayland.outputs[1] and wayland.outputs[1].name or nil
    local cells = {}
    self.workspace_ids = {}
    local pos = 0
    for _, ws in ipairs(wayland.workspaces) do
        if not output or ws.monitor == output then
            pos = pos + 1
            self.workspace_ids[pos] = ws.id
            local label = wayland.active_workspace == ws.id and ("[" .. pos .. "]") or tostring(pos)
            cells[#cells + 1] = ui.button(label, "ws:" .. pos)
        end
    end
    if #cells == 0 then return ui.text("--") end
    return ui.row(cells)
end

function app:on_action(key)
    local pos = key:match("^ws:(%d+)$")
    local id = pos and self.workspace_ids[tonumber(pos)]
    if id then os.execute("hyprctl dispatch 'hl.dsp.focus({workspace = " .. id .. "})' >/dev/null 2>&1") end
end

return app
