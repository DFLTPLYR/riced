-- Configure API_KEY in the installed copy. HTTP uses native shell I/O
-- until M4's task-based network effects are available.
local API_KEY = ""
local URL = "https://api.cline.bot/api/v1/users/me/plan/usage-limits"
local app = { usage = nil, last_fetch = 0 }

function app:view()
    if API_KEY ~= "" and (not self.usage or os.time() - self.last_fetch > 3600) then
        local handle = io.popen("curl -sS --max-time 10 -H 'Authorization: Bearer " .. API_KEY .. "' " .. URL .. " 2>/dev/null")
        if handle then
            local body = handle:read("*a") or ""
            handle:close()
            if body ~= "" then
                self.usage = body
                self.last_fetch = os.time()
            elseif not self.usage then
                self.usage = '{"error":"fetch failed"}'
            end
        elseif not self.usage then
            self.usage = '{"error":"fetch failed"}'
        end
    end
    return ui.icon("bot")
end

function app:popup()
    if API_KEY == "" then
        return { ui = ui.card({ title = "Cline Pass", icon = "bot", body = "paste API_KEY into clinepass.lua" }), width = 300 }
    end
    if not self.usage then
        return { ui = ui.card({ title = "Cline Pass", icon = "bot", body = ui.row({ ui.spinner(), ui.text("fetching…") }) }), width = 300 }
    end
    if self.usage:match('"error"') then return { text = "cline: unauthorized (bad key?)", width = 300 } end
    local rows = {}
    for kind, pct in self.usage:gmatch('"type"%s*:%s*"([%w_%-]+)"%s*,%s*"percentUsed"%s*:%s*(%d+)') do
        local p = tonumber(pct) or 0
        rows[#rows + 1] = ui.row({
            ui.text(kind:gsub("_", " ")):width(80),
            ui.progress(p / 100):height(8):width("fill"),
            ui.text(p .. "%"):width(20),
        })
    end
    if #rows == 0 then rows[1] = ui.text("no usage fields parsed") end
    return { ui = ui.card({ title = "Cline Pass", icon = "bot", color = theme.primary, body = ui.column(rows) }), width = 300 }
end

return app
