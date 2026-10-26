local app = { defaults = { height = 32 }, clicks = 0 }

function app:mount(context)
	self.output = context.output.name
	self.clock = os.date("%H:%M:%S")
	self:every(1000, function()
		self.clock = os.date("%H:%M:%S")
	end)
end

function app:actions(event)
	if event.key == "increment" then
		self.clicks = self.clicks + 1
	end
end

function app:view(props)
	-- Three aligned slots like the legacy bar: start, flexible center, end.
	return iced.layer({
		id = "main",
		layer = "top",
		anchors = { top = true, left = true, right = true },
		height = props.height,
		exclusive_zone = props.height,
		keyboard = "none",
		input = "all",
		content = iced.container(iced.row({
			iced.row({
				riced.component("status.lua", { output = self.output }),
			}):spacing(8),
			iced.space():width(size.fill),
			iced.row({
				iced.text(self.clock),
			}):spacing(8),
			iced.space():width(size.fill),
			iced.row({
				iced.button("Clicks: " .. self.clicks, "increment"),
			}):spacing(8),
		})
			:spacing(12)
			:height(size.fill)
			:align_y("center"))
			:padding({ left = 12, right = 12 })
			:background(theme.surface)
			:width(size.fill)
			:height(size.fill),
	})
end

return app
