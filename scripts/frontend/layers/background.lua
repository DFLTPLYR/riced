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
	local s = props.selection
	if s and s.width > 1 and s.height > 1 then
		local x, y = s.x - props.output.x, s.y - props.output.y
		local color = theme.primary:sub(1, 7)
		-- Draw the full global rectangle; clipping must not invent borders at monitor seams.
		children[#children + 1] = iced.canvas({
			rects = { { x = x, y = y, width = s.width, height = s.height, color = color .. "80" } },
			lines = {
				{
					points = {
						{ x, y },
						{ x + s.width, y },
						{ x + s.width, y + s.height },
						{ x, y + s.height },
						{ x, y },
					},
					color = color,
					width = 1,
				},
			},
		})
			:width(size.fill)
			:height(size.fill)
			:animate({
				id = "desktop-selection-" .. s.id,
				opacity = iced.animations.opacity({
					from = 1,
					to = props.selecting and 1 or 0,
					duration = props.selecting and 0 or (props.fade_ms or 200),
					easing = "ease_in_out",
				}),
			})
	end
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
		shared_pointer_events = true,
		anchors = { top = true, right = true, bottom = true, left = true },
		content = iced.stack(children):width(size.fill):height(size.fill),
		popup = popup,
	})
end
return layer
