-- Pure notification card. The shell owns notification state; this builder
-- receives owned metadata and never stores per-render data.
--
-- The module shim at the end keeps legacy `riced.component(...)` callers
-- working while the runtime-only layers/notifications.lua path uses
-- `iced.notification_card(n)` directly.
local function build(n)
	local app = n.app or ""
	local title = n.title or ""
	local urgency = n.urgency or 0
	local head = app ~= "" and (app .. " — " .. title) or title
	if urgency >= 2 then
		head = "! " .. head
	end
	local icon
	if not n.has_image then
		icon = "bell"
	end
	local content = iced.card({
		title = head,
		icon = icon,
		color = urgency >= 2 and (theme and theme.error) or nil,
		body = iced.text(n.body or ""):wrap("word-or-glyph"):elide(50),
		background = theme and theme.surface or nil,
		padding = 12,
		radius = 10,
	})
	if #(n.actions or {}) == 0 then
		return content
	end
	local actions = {}
	for _, action in ipairs(n.actions) do
		actions[#actions + 1] = iced.button(action.label, "invoke:" .. n.id .. ":" .. action.key)
	end
	return iced.column({ content, iced.row(actions) })
end

if iced.notification_card == nil then
	iced.define("notification_card", build)
end

return {
	view = function(_, n)
		return iced.notification_card(n)
	end,
}
