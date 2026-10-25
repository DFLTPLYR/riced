return {
    view = function(self, props)
        return iced.text("Riced / " .. props.output):color(theme.primary)
    end,
}
