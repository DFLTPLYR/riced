-- Example Lua plugin. A layer embeds it with
-- `riced.plugin("hello", { name = "world" })`; the host resolves
-- `plugins/hello.lua` and renders `view` like a widget leaf.
return {
    view = function(self, props)
        props = props or self.props or {}
        return iced.container(iced.text("Hello " .. (props.name or "plugin")):color(theme.on_surface))
            :padding(12)
            :background(theme and theme.surface)
            :radius(10)
    end,
}
