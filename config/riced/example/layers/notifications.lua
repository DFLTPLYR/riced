-- Notification popup layer. The Rust service owns delivery, expiry, and the
-- notification queue; this layer is the whole visual surface and can be
-- removed or replaced without touching the service.
local layer = {}

local function items_for(props)
	local output = props.output and props.output.name
	local items = {}
	for _, n in ipairs(notifications or {}) do
		if n.popup_visible and n.output == output then
			items[#items + 1] = n
		end
	end
	return items
end

local function card_for(n)
	-- Prefer the local `components/notification.lua` builder. Keep the
	-- seeded card as a fallback for fresh installs.
	-- if iced.notification then
	-- 	return iced.notification(n)
	-- end
	if iced.notification_card then
		return iced.notification_card(n)
	end
	return riced.component("notification_card.lua", n)
end

function layer:view(props)
	local items = items_for(props)
	local available = 0
	if props.output and props.output.height then
		available = math.floor(tonumber(props.output.height) or 0)
	end
	local height = math.floor(props.height or 480)
	if available > 0 then
		height = math.floor(math.min(height, math.max(1, available - 32)))
	end
	height = math.max(1, height)
	return iced.layer({
		id = "notifications",
		layer = "overlay",
		anchors = { top = true, right = true },
		width = math.tointeger(props.output.width) or 0,
		height = math.tointeger(props.output.height) or 0,
		input = "regions",
		margin = { top = 16, right = 16 },
		content = iced.listview(items)
			:id("notifications")
			:key("id")
			:delegate(function(n)
				local card = card_for(n)
				-- Explicit input region: the layer can be larger than the
				-- cards, but only the card bounds capture clicks.
				return iced.region(card):on_press("dismiss:" .. n.id)
			end)
			:onEntered({ x = { from = 60, to = 0 }, opacity = { from = 0, to = 1 }, duration = 250 })
			:onExit({ x = { to = 60 }, opacity = { to = 0 }, duration = 250 })
			:onDisplaced({ duration = 250 })
			:width(360)
			:height(size.fill)
			:anchor("top-right"),
	})
end

return layer
