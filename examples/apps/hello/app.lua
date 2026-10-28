-- Consumes the supervised `hello` process extension built from
-- `extensions/hello`. The service is optional: the card reports availability
-- instead of failing when the worker executable is missing.
local app = {}

function app:mount()
	self.hello = riced.service("hello")
	local snapshot = self.hello:snapshot()
	self.greeting = snapshot.data.greeting
	self.available = snapshot.available
	self.hello:subscribe(function(snapshot)
		self.greeting = snapshot.data.greeting
		self.available = snapshot.available
	end)
end

function app:actions(event)
	if event.type == "action" and event.key == "greet" then
		self.pending = true
		self.hello:command("greet", { name = "riced" }, function(result)
			self.pending = false
			if result.ok then
				self.greeting = result.data.greeting
				self.error = nil
			else
				self.error = result.error.message
			end
		end)
	end
end

function app:view()
	local colors = theme
	local status
	if self.error then
		status = self.error
	elseif self.pending then
		status = "Greeting…"
	elseif self.available then
		status = "Service available"
	else
		status = "Unavailable — build extensions/hello"
	end
	return iced.layer({
		id = "hello",
		layer = "top",
		anchors = { top = true, left = true },
		width = 260,
		height = 140,
		margin = { top = 116, left = 16 },
		content = iced.container(iced.column({
			iced.text("Hello extension"):size(15):color(colors.on_surface),
			iced.text(self.greeting or ""):color(colors.primary),
			iced.text(status):size(11):wrap("word"):color(colors.on_surface),
			iced.button("Greet", "greet"),
		}):spacing(10))
			:padding(16)
			:width(size.fill)
			:height(size.fill)
			:background(colors.surface)
			:radius(12),
	})
end

return app
