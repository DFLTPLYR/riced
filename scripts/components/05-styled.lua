-- Pure styled primitives. Names avoid shadowing the raw ui constructors.
ui.define("surface", function(props)
    props = props or {}
    local body = props.body or ui.space()
    if type(body) == "string" then body = ui.text(body) end
    local node = ui.container(body)
    for _, key in ipairs({ "width", "height", "padding", "background", "radius", "border", "border_width" }) do
        if props[key] ~= nil then node = node[key](node, props[key]) end
    end
    return node
end)

ui.define("styled_button", function(props)
    props = props or {}
    local node = ui.button(props.label or "", props.action or "")
    for _, key in ipairs({ "width", "height", "padding", "color", "background", "radius" }) do
        if props[key] ~= nil then node = node[key](node, props[key]) end
    end
    return node
end)

ui.define("styled_progress", function(props)
    props = props or {}
    local node = ui.progress(props.value or 0)
    for _, key in ipairs({ "width", "height", "color", "background" }) do
        if props[key] ~= nil then node = node[key](node, props[key]) end
    end
    return node
end)

ui.define("styled_separator", function(props)
    props = props or {}
    local node = ui.separator()
    if props.height ~= nil then node = node:height(props.height) end
    if props.color ~= nil then node = node:color(props.color) end
    return node
end)
