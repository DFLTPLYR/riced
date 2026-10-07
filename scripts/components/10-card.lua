-- Card is a pure builder. Optional styling props request a container surface.
ui.define("card", function(props)
    props = props or {}
    local body = props.body
    if type(body) == "string" then body = ui.text(body) end
    local title = ui.text(props.title or ""):size(props.title_size or 14)
    if props.color then title = title:color(props.color) end
    if props.icon then
        local icon = ui.icon(props.icon)
        if props.color then icon = icon:color(props.color) end
        title = ui.row({ icon, title })
    end
    local children = { title, ui.separator(), body or ui.space() }
    if props.footer then children[#children + 1] = props.footer end
    local content = ui.column(children):spacing(props.spacing or 4)
    if props.background or props.padding or props.radius then
        return ui.container(content):padding(props.padding or 10):radius(props.radius or 6):background(props.background)
    end
    return content
end)
