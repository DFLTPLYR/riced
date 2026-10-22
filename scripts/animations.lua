local animations = iced.animations
local app = { expanded = false }

function app:view()
    return iced.column({
        iced.button("Toggle animation", "toggle"),
        iced.container(iced.text("Rust animates this card; Lua only changes its targets."))
            :padding(16)
            :background("#34495e")
            :animate({
                id = "demo-card",
                y = animations.y({
                    to = self.expanded and 0 or 24,
                    duration = 300,
                    easing = "ease_in_out",
                }),
                opacity = animations.opacity({
                    from = 0,
                    to = self.expanded and 1 or 0.25,
                    duration = 250,
                }),
            }),
    }, 12)
end

function app:actions(event)
    if event.key == "toggle" then
        self.expanded = not self.expanded
    end
end

return app
