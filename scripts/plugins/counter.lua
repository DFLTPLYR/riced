-- Example interactive Lua plugin. Action keys are routed back to this module
-- as `plugin:counter:<key>` by the host.
return {
    count = 0,
    view = function(self)
        return iced.row({
            iced.text("count " .. self.count):color(theme.on_surface),
            iced.button("+1", "inc"):padding(6),
            iced.button("reset", "reset"):padding(6),
        }):spacing(8)
    end,
    actions = function(self, event)
        if event.type ~= "action" then
            return
        end
        if event.key == "inc" then
            self.count = self.count + 1
        elseif event.key == "reset" then
            self.count = 0
        end
    end,
}
