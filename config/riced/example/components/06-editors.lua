-- Presentation only: apps own selection, file dialogs, and persistence.
iced.define("color_swatch", function(props)
    props = props or {}
    local node = iced.container(iced.space())
        :width(props.width or 32):height(props.height or 24)
        :radius(props.radius or 4):background(props.color or theme.primary)
    if props.action then node = node:on_press(props.action) end
    return node
end)

iced.define("color_row", function(props)
    props = props or {}
    return iced.row({
        iced.color_swatch({ color = props.color, action = props.action }),
        iced.text(props.label or ""):width(size.fill),
        iced.text(props.color or ""),
    }):spacing(8):width(size.fill)
end)

iced.define("image_preview", function(props)
    props = props or {}
    local body
    if props.image then
        body = iced.image_handle(props.image):width(props.width or 240):height(props.height or 140)
    elseif props.path and props.path ~= "" then
        body = iced.image(props.path):width(props.width or 240):height(props.height or 140)
    else
        body = iced.text(props.placeholder or "No image selected")
    end
    return iced.container(body):width(props.width or 240):height(props.height or 140)
        :align_x("center"):align_y("center")
end)

iced.define("file_row", function(props)
    props = props or {}
    return iced.row({
        iced.text(props.path or "No file selected"):width(size.fill):elide(48),
        iced.button(props.label or "Choose file", props.action or "choose_file"),
    }):spacing(8):width(size.fill)
end)

iced.define("page_tabs", function(props)
    props = props or {}
    local children = {}
    local seen = {}
    for _, page in ipairs(props.pages or {}) do
        assert(type(page.id) == "string" and page.id ~= "", "page_tabs needs nonempty string ids")
        assert(not seen[page.id], "page_tabs ids must be unique")
        seen[page.id] = true
        local label = page.label or page.id
        if page.id == props.selected then label = "• " .. label end
        table.insert(children, iced.button(label, (props.prefix or "page:") .. page.id))
    end
    if props.vertical then return iced.column(children):spacing(4) end
    return iced.row(children):spacing(4)
end)
