local layer = {}
function layer:view(props)
	local children = {
		iced.container(iced.space()):width(size.fill):height(size.fill):background(theme.background),
	}
	-- The source Wallpaper Settings saved. Drawn directly so a background is
	-- visible immediately and still visible if the bake fails; iced.image uses
	-- contain fitting, so this is a fallback rather than the final look.
	if props.wallpaper and props.wallpaper ~= "" then
		children[#children + 1] = iced.image(props.wallpaper):width(size.fill):height(size.fill)
	end
	-- The baked tile (cover fit, shader-aware) published after Apply. Empty
	-- until the first bake completes, so it only ever draws over the above.
	children[#children + 1] = riced.plugin("wallpaper")
	children[#children + 1] = riced.plugin("desktop-widgets")
	local popup
	if props.menu then
		popup = {
			id = "background-menu",
			x = props.menu.x,
			y = props.menu.y,
			width = 180,
			height = 96,
			content = iced.container(iced.column({
				iced.button("Add bar", "add-bar"):width(size.fill),
				iced.button("Settings", "settings"):width(size.fill),
			}):spacing(6))
				:padding(8)
				:background(theme.surface)
				:width(size.fill)
				:height(size.fill),
		}
	end
	return iced.layer({
		id = "background",
		layer = "background",
		exclusive_zone = -1,
		pointer_events = true,
		anchors = { top = true, right = true, bottom = true, left = true },
		content = iced.stack(children):width(size.fill):height(size.fill),
		popup = popup,
	})
end
return layer
