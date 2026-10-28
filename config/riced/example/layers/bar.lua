-- A bar surface declared in Lua. The slot list is data: every entry is a row
-- of `riced.widget` instances, so a bar can have any number of regions.
local layer = {}

local function slot_view(slot, index)
	local items = {}
	for position, name in ipairs(slot.widgets or {}) do
		local id = (slot.align or "center") .. "-" .. name .. "-" .. index .. "-" .. position
		items[#items + 1] = riced.widget(name, {}, id)
	end
	return iced.row(items):spacing(8)
end

function layer:view(props)
	local edge = props.edge or "top"
	local thickness = props.thickness or 34
	local horizontal = edge == "top" or edge == "bottom"
	local anchors = { [edge] = true }
	if horizontal then
		anchors.left, anchors.right = true, true
	else
		anchors.top, anchors.bottom = true, true
	end
	local content = {}
	for index, slot in ipairs(props.slots or {}) do
		if index > 1 then
			content[#content + 1] = horizontal and iced.space():width(size.fill) or iced.space():height(size.fill)
		end
		content[#content + 1] = slot_view(slot, index)
	end
	return iced.layer({
		id = props.id,
		layer = "top",
		anchors = anchors,
		exclusive_zone = thickness,
		width = not horizontal and thickness or nil,
		height = horizontal and thickness or nil,
		pointer_events = true,
		popup = riced.widget_popup({
			edge = edge,
			thickness = thickness,
			output = props.output,
		}),
		content = iced.container(
			horizontal and iced.row(content):spacing(12):align_y("center") or iced.column(content):spacing(12)
		)
			:padding(6)
			:background(theme.surface)
			:width(size.fill)
			:height(size.fill),
	})
end

return layer
