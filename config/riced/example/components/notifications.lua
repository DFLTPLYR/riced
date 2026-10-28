-- riced:composable
-- A whole scene, rendered by the same overlay host as other Lua sources.
local app = {}
function app:view(props)
	return iced.listview(props.notifications)
		:id("notifications")
		:key("id")
		:delegate(function(n)
			local content = iced.notification(n)
			return content:on_press("dismiss:" .. n.id)
		end)
		:onEntered({ x = { from = 200, to = 0 }, opacity = { from = 0, to = 1 }, duration = 250 })
		:onExit({ x = { to = -200 }, opacity = { to = 0 }, duration = 250 })
		:onDisplaced({ duration = 250 })
		:width(360)
		:anchor("top-right")
		:margin(16)
end
function app:actions(events)
	if events.type ~= "action" then
		return
	end
	local key = events.key
	local id = key:match("^dismiss:(%d+)$")
	if id then
		return { dismiss = tonumber(id) }
	end
	local target, action = key:match("^invoke:(%d+):(.*)$")
	if target then
		return { invoke = { id = tonumber(target), key = action } }
	end
end
return app
