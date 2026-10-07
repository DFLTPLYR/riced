-- Shell polling runs here until M4 provides task-based effects.
local app = { workspace_ids = {} }

function app:view()
    local ids_h = io.popen("hyprctl workspaces -j 2>/dev/null")
    if not ids_h then return ui.text("--") end
    local ids = ids_h:read("*a") or ""
    ids_h:close()
    local active_h = io.popen("hyprctl activeworkspace -j 2>/dev/null")
    local active = active_h and active_h:read("*a") or nil
    if active_h then active_h:close() end
    local output = active and active:match('"monitor"%s*:%s*"([^"]+)"') or nil
    local current = active and active:match('"id"%s*:%s*(%d+)') or nil
    local cells = {}
    self.workspace_ids = {}
    local pos = 0
    for id, mon in ids:gmatch('"id"%s*:%s*(%d+)[%s%S]-"monitor"%s*:%s*"([^"]+)"') do
        if not output or mon == output then
            pos = pos + 1
            self.workspace_ids[pos] = id
            local label = id == current and ("[" .. pos .. "]") or tostring(pos)
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
