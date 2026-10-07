-- Notification app:view(n) receives owned metadata supplied by the shell.
local app = {}

function app:view(n)
    local head = n.app ~= "" and (n.app .. " — " .. n.title) or n.title
    if n.urgency >= 2 then head = "! " .. head end
    local icon
    if not n.has_image then icon = "bell" end
    local content = ui.card({
        title = head, icon = icon,
        color = n.urgency >= 2 and theme.error or nil,
        body = ui.text(n.body),
    })
    if #(n.actions or {}) == 0 then return content end
    local actions = {}
    for _, action in ipairs(n.actions) do
        actions[#actions + 1] = ui.button(action.label, action.key)
    end
    return ui.column({ content, ui.row(actions) })
end

function app:transitions()
    return {
        add = { x = { from = 200, to = 0 }, opacity = { from = 0, to = 1 }, duration = 250 },
        remove = { x = { to = -200 }, opacity = { to = 0 }, duration = 250 },
        displaced = { duration = 250 },
    }
end

return app
