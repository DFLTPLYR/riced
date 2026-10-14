-- Card is a pure builder. Optional styling props request a container surface.
iced.define("card", function(props)
    props = props or {}
    local body = props.body
    if type(body) == "string" then body = iced.text(body) end
    local title = iced.text(props.title or ""):size(props.title_size or 14)
    if props.color then title = title:color(props.color) end
    if props.icon then
        local icon = iced.icon(props.icon)
        if props.color then icon = icon:color(props.color) end
        title = iced.row({ icon, title })
    end
    local children = { title, iced.styled_separator({ color = props.separator_color }), body or iced.space() }
    if props.footer then children[#children + 1] = props.footer end
    local content = iced.column(children):spacing(props.spacing or 4)
    if props.background or props.padding or props.radius or props.border or props.border_width then
        return iced.surface({
            body = content, padding = props.padding or 10, radius = props.radius or 6,
            background = props.background, border = props.border, border_width = props.border_width,
        })
    end
    return content
end)
