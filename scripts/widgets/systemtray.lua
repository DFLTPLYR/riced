local app = { selected = nil, path = {} }

local function selected(self)
	for _, item in ipairs(systemtray.items) do
		if item.id == self.selected then
			return item
		end
	end
end

local function node_at(self, item)
	local node = item.menu
	for _, id in ipairs(self.path) do
		local next_node = nil
		for _, child in ipairs(node and node.children or {}) do
			if child.id == id then
				next_node = child
			end
		end
		node = next_node
	end
	return node
end

function app:view()
	local icons = {}
	for _, item in ipairs(systemtray.items) do
		if item.status ~= "Passive" then
			local icon = item.icon_pixmap or item.icon_path
			if item.status == "NeedsAttention" then
				icon = item.attention_icon or icon
			end
			local content = icon and iced.image(icon):width(20):height(20) or iced.icon("circle")
			icons[#icons + 1] = content:on_press("item|" .. item.id)
		end
	end
	return iced.row(icons):spacing(8)
end

function app:popup()
	local item = selected(self)
	if not item then
		return iced.popup(iced.text("Tray item unavailable")):width(260)
	end
	local rows = {}
	if #self.path > 0 then
		rows[#rows + 1] = iced.button("Back", "back")
	end
	rows[#rows + 1] = iced.button("Activate", "activate")
	rows[#rows + 1] = iced.button("Application context menu", "context")
	local node = node_at(self, item)
	for _, entry in ipairs(node and node.children or {}) do
		if entry.visible then
			if entry.type == "separator" then
				rows[#rows + 1] = iced.text("────────────")
			elseif entry.enabled then
				local label = entry.label:gsub("__", "\1"):gsub("_", ""):gsub("\1", "_")
				if entry.toggle_type ~= "" then
					label = (entry.checked and "[x] " or "[ ] ") .. label
				end
				if entry.submenu then
					label = label .. " >"
				end
				rows[#rows + 1] = iced.button(label, "entry|" .. entry.id):width("fill")
			else
				rows[#rows + 1] = iced.text(entry.label)
			end
		end
	end
	local body = iced.scrollable(iced.container(iced.column(rows):spacing(4)):background(theme.surface))
		:height(360)
		:scrollbar("hidden")
	return iced.popup(body):width(150)
end

function app:actions(events)
	if events.type ~= "action" then
		return
	end
	local key = events.key
	local id = key:match("^item|(.+)$")
	if id then
		self.selected, self.path = id, {}
		local item = selected(self)
		if item and item.has_menu then
			return tray.open(id)
		end
		return tray.activate(id)
	end
	local item = selected(self)
	if not item then
		return
	end
	if key == "activate" then
		return tray.activate(item.id)
	end
	if key == "context" then
		return tray.context(item.id)
	end
	if key == "back" then
		table.remove(self.path)
		return tray.aboutToShow(item.id, self.path[#self.path] or 0)
	end
	local entry_id = tonumber(key:match("^entry|(-?%d+)$"))
	local node = node_at(self, item)
	for _, entry in ipairs(node and node.children or {}) do
		if entry.id == entry_id and entry.enabled and entry.visible then
			if entry.submenu then
				self.path[#self.path + 1] = entry.id
				return tray.aboutToShow(item.id, entry.id)
			end
			return tray.menuEvent(item.id, entry.id)
		end
	end
end

return app
