-- Shell entry point: every surface is declared here.
--
-- Bars are plain data files under `data/bars/`. shell.lua lists the directory
-- and declares one layer per file; Settings → Panel reads and writes the same
-- files. Nothing here hardcodes a bar or its widgets.
local state = require("data.state")
local settings = require("components.settings.window")

local app = {}

local function default_slots(edge)
	if edge == "top" or edge == "bottom" then
		return {
			{ align = "start", widgets = { "workspaces" } },
			{ align = "center", widgets = { "clock" } },
			{ align = "end", widgets = { "systemtray", "notifycenter", "system" } },
		}
	end
	return {
		{ align = "start", widgets = { "workspaces" } },
		{ align = "center", widgets = { "clock" } },
		{ align = "end", widgets = { "system" } },
	}
end

function app.new()
	return app
end
function app.namespace()
	return "riced"
end

function app:add_bar(options)
	local path = "data/bars/" .. state.bar_name(options)
	if pcall(riced.read, path) then
		error("that edge already has a bar")
	end
	riced.write(path, {
		edge = options.anchor,
		output = options.output,
		thickness = options.thickness or 34,
		slots = default_slots(options.anchor),
	})
end

function app:remove_bar(options)
	riced.remove("data/bars/" .. state.bar_name(options))
end

local function output_view(self, props)
	local prefix = props.output.name:gsub(".", function(byte)
		return string.format("%02x", string.byte(byte))
	end) .. "."
	local layers = {
		riced.layer("layers/background.lua", {
			output = props.output,
			wallpaper = props.wallpaper,
			selection = self.selection,
			selecting = self.selecting,
			fade_ms = props.selection_fade_ms,
		}),
		-- Notification popups are ordinary Lua layers. The service still owns
		-- delivery and expiry; removing this line leaves notifications headless.
		riced.layer("layers/notifications.lua", {
			output = props.output,
		}),
	}
	for _, name in ipairs(riced.list("data/bars")) do
		local ok, bar = pcall(riced.read, "data/bars/" .. name)
		if ok and type(bar) == "table" and (not bar.output or bar.output == "" or bar.output == props.output.name) then
			local stem = name:gsub("%.lua$", "")
			layers[#layers + 1] = riced.layer("layers/bar.lua", {
				id = prefix .. "bar-" .. stem,
				edge = bar.edge or "top",
				thickness = bar.thickness or 34,
				slots = bar.slots or {},
				output = props.output,
			})
		end
	end
	for _, layer in ipairs(layers) do
		layer.output = props.output.name
		layer.id = layer.id or "main"
		if layer.id:sub(1, #prefix) ~= prefix then
			layer.id = prefix .. layer.id
		end
	end
	return layers
end

function app:view(props)
	local layers = {}
	for _, output in ipairs(props.outputs or {}) do
		for _, layer in
			ipairs(output_view(self, {
				output = output,
				wallpaper = output.wallpaper,
				selection_fade_ms = props.selection_fade_ms,
			}))
		do
			layers[#layers + 1] = layer
		end
	end
	if self.settings_open then
		layers[#layers + 1] = settings:view()
	end
	return layers
end

function app:update(event)
	if event.type == "window_closed" and event.id == "settings" then
		self.settings_open = false
		return
	end
	if event.layer and event.layer:match("%.background$") then
		local output = event.output
		if event.type == "press" and event.button == "left" then
			self.menu = nil
			self.selection_id = (self.selection_id or 0) + 1
			local x, y = event.x + output.x, event.y + output.y
			self.selection = { id = self.selection_id, start_x = x, start_y = y, x = x, y = y, width = 0, height = 0 }
			self.selecting = true
		elseif self.selecting then
			local released = event.type == "release" and event.button == "left"
			if (event.type == "move" or released) and event.x and event.y then
				local s = self.selection
				local x, y = event.x + output.x, event.y + output.y
				s.x, s.y = math.min(s.start_x, x), math.min(s.start_y, y)
				s.width, s.height = math.abs(x - s.start_x), math.abs(y - s.start_y)
			end
			if released or event.type == "cancel" then
				self.selecting = false
			end
		end
	end
	if not event.key then
		return
	end
	if event.key == "settings" then
		self.settings_open = not self.settings_open
		self.menu = nil
		return
	end
	if event.key == "dismiss-all" then
		return notification.dismissAll()
	end
	local dismissed = event.key:match("^dismiss:(%d+)$")
	if dismissed then
		return notification.dismiss(tonumber(dismissed))
	end
	local target, key = event.key:match("^invoke:(%d+):(.*)$")
	if target then
		return { invoke = { id = tonumber(target), key = key } }
	end
end

function app:title(surface)
	return surface.title
end
function app:subscription(active)
	local sources = { "input", "wayland", "config", "ipc", "tray" }
	for _, name in ipairs({
		"desktop",
		"apps",
		"settings",
		"animation",
		"notification_deadlines",
		"overlays",
		"notifications",
		"selection",
		"widgets",
	}) do
		if active[name] then
			sources[#sources + 1] = name
		end
	end
	return sources
end
function app:theme(context)
	return context.theme
end
function app:style(context)
	return { background_color = "#00000000", text_color = context.text_color }
end
function app:redraw_scope(event)
	return event.scope
end

return riced
	.daemon(app.new, app.namespace, app.update, app.view)
	:title(app.title)
	:subscription(app.subscription)
	:theme(app.theme)
	:style(app.style)
	:redraw_scope(app.redraw_scope)
	:settings({
		id = "riced",
		layer_settings = { start_mode = "background", keyboard_interactivity = "none" },
	})
	:run()
