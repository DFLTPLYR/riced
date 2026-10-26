local component = {}
function component:view(props)
    return iced.container(iced.column({
        iced.button("Choose wallpaper", "pick-wallpaper"):width(size.fill),
        iced.button("Reload / discard edits", "reload"):width(size.fill),
        iced.button("Add Top", "add-top"):width(size.fill),
        iced.button("Remove Top", "remove-bar"):width(size.fill),
        iced.button("Open Settings", "open-settings"):width(size.fill),
        iced.button("Close", "close-menu"):width(size.fill),
    }):spacing(6))
        :padding(8):width(props.width):height(props.height)
        :background(theme.surface):border(theme.primary):border_width(1):radius(6)
end
return component
