-- Wallpaper draft editor. The map canvas is a host-filled native slot; Rust
-- owns draft state, previews, baking, and persistence.
local app = {}

local function hint(text)
	return iced.text(text):size(12):wrap("word"):color(theme.on_surface)
end

local function heading(title, description)
	return iced.column({
		iced.text(title):size(16):color(theme.on_surface),
		hint(description),
	})
		:spacing(5)
		:width(size.fill)
end

local function choice(label, action, selected)
	return iced.button(label, action)
		:padding(8)
		:background(selected and theme.primary or theme.surface)
		:color(selected and theme.on_primary or theme.on_surface)
end

local function image_controls(props)
	local rows = {}
	if #props.images == 0 then
		table.insert(rows, iced.text("No images yet."):size(13):color(theme.on_surface))
		table.insert(rows, hint("Add wallpaper… to place the first one."))
		return rows
	end
	local picker = {}
	for _, image in ipairs(props.images) do
		table.insert(picker, choice(image.label, image.action, image.selected))
	end
	table.insert(rows, iced.row(picker):spacing(8))
	table.insert(
		rows,
		iced.row({
			iced.text(props.image.caption):size(13):width(size.fill):color(theme.on_surface),
			iced.button("Remove image", "remove-image"):padding(6):color(theme.error),
		})
			:spacing(8)
			:width(size.fill)
	)
	table.insert(rows, hint(props.image.resolution))
	table.insert(
		rows,
		heading(
			"Placement",
			"X/Y use desktop coordinates. Higher Z paints in front; scale multiplies width and height."
		)
	)
	for _, field in ipairs(props.image.fields) do
		table.insert(
			rows,
			iced.container(iced.column({
				iced.text(field.label):size(14):width(size.fill):color(theme.on_surface),
				iced.row({
					iced.spin_box(field.value, field.min, field.max)
						:step(field.step)
						:decimals(field.decimals)
						:on_change(field.action)
						:width(size.fill),
					iced.button("Reset default", field.reset):padding(6),
				})
					:spacing(12)
					:width(size.fill),
			}):spacing(8))
				:padding(10)
				:width(size.fill)
		)
	end
	return rows
end

local function monitor_controls(props)
	local rows = {}
	if type(props.monitor) ~= "table" then
		table.insert(rows, hint("No displays connected."))
		return rows
	end
	local picker = {}
	for _, monitor in ipairs(props.monitors) do
		table.insert(picker, choice(monitor.key, monitor.action, monitor.selected))
	end
	table.insert(rows, iced.row(picker):spacing(8))
	table.insert(rows, iced.text(props.monitor.caption):size(13):color(theme.on_surface))
	table.insert(rows, hint(props.monitor.path_label))
	table.insert(
		rows,
		iced.row({
			iced.button("Choose wallpaper…", "pick-monitor"):padding(8),
			iced.button("Clear wallpaper", "clear-monitor"):padding(8),
		}):spacing(8)
	)
	table.insert(rows, iced.text("Content fit"):size(13):color(theme.on_surface))
	local fits = {}
	for _, fit in ipairs(props.monitor.fits) do
		table.insert(fits, choice(fit.label, fit.action, fit.selected))
	end
	table.insert(rows, iced.row(fits):spacing(8))
	table.insert(
		rows,
		hint(
			"Fit: whole image · Expand: fill and crop · Stretch: fill without preserving aspect ratio · Center: native pixels. All placements are centered."
		)
	)
	table.insert(rows, hint(props.monitor.disconnected_hint))
	return rows
end

function app:view(props)
	local rows = {}
	local modes = {}
	for _, mode in ipairs(props.modes) do
		table.insert(modes, choice(mode.label, mode.action, mode.selected))
	end
	table.insert(rows, iced.row(modes):spacing(8))
	table.insert(rows, heading(props.heading_title, props.heading_description))
	local actions = { iced.button("Add wallpaper…", props.add):padding(8) }
	table.insert(actions, iced.space():width(size.fill))
	table.insert(actions, hint(props.status))
	table.insert(actions, iced.button("Discard", "discard"):padding(8))
	table.insert(actions, iced.button(props.apply_label, props.apply):padding(8))
	table.insert(rows, iced.row(actions):spacing(8):width(size.fill))
	table.insert(rows, riced.plugin("wallpaper-map"))
	local controls = props.per_monitor and monitor_controls(props) or image_controls(props)
	for _, row in ipairs(controls) do
		table.insert(rows, row)
	end
	if props.loading then
		table.insert(rows, hint("Loading full-resolution originals…"))
	end
	if type(props.error) == "string" then
		table.insert(rows, iced.text(props.error):size(12):wrap("word"):color(theme.error))
	end
	return iced.column(rows):spacing(16):width(size.fill)
end

return app
