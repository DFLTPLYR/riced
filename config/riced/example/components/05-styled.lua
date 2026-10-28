-- Pure styled primitives. Names avoid shadowing the raw ui constructors.
iced.define("surface", function(props)
    props = props or {}
    local body = props.body or iced.space()
    if type(body) == "string" then body = iced.text(body) end
    local node = iced.container(body)
    for _, key in ipairs({ "width", "height", "padding", "background", "radius", "border", "border_width" }) do
        if props[key] ~= nil then node = node[key](node, props[key]) end
    end
    return node
end)

iced.define("styled_button", function(props)
    props = props or {}
    local node = iced.button(props.label or "", props.action or "")
    for _, key in ipairs({ "width", "height", "padding", "color", "background", "radius" }) do
        if props[key] ~= nil then node = node[key](node, props[key]) end
    end
    return node
end)

iced.define("styled_progress", function(props)
    props = props or {}
    local node = iced.progress(props.value or 0)
    for _, key in ipairs({ "width", "height", "color", "background" }) do
        if props[key] ~= nil then node = node[key](node, props[key]) end
    end
    return node
end)

iced.define("styled_separator", function(props)
    props = props or {}
    local node = iced.separator()
    if props.height ~= nil then node = node:height(props.height) end
    if props.color ~= nil then node = node:color(props.color) end
    return node
end)
