-- Native systemctl dispatch; task-based effects arrive in M4.
local app = {}
local allowed = { suspend = true, hibernate = true, reboot = true, poweroff = true }

function app:view()
	return iced.icon("power")
end

function app:popup()
	return iced.popup(iced.card({
		title = "Session",
		icon = "power",
		body = iced.menu({
			id = "session-actions",
			items = {
				{ label = "Suspend", action = "suspend" },
				{ label = "Hibernate", action = "hibernate" },
				{ label = "Reboot", action = "reboot" },
				{ label = "Power off", action = "poweroff" },
			},
		}),
		background = theme and theme.surface,
		padding = 8,
		radius = { bottom_right = 12, bottom_left = 12 },
	}))
		:width(100)
		:height(200)
end

function app:actions(events)
	if events.type == "click" and events.button == "left" then
		self:popup()
		return
	end
	if events.type ~= "action" then
		return
	end
	local key = events.key
	if allowed[key] then
		os.execute("systemctl " .. key)
	end
end

return app
