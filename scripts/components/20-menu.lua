-- Reusable keyed ListView. Give each menu in a tree a distinct props.id.
ui.define("menu", function(props)
    props = props or {}
    local list = ui.listview(props.items or {})
        :id(props.id or "menu"):key("action"):pitch(props.pitch or 32):spacing(props.spacing or 4)
        :delegate(function(item)
            local button = ui.button(item.label or "?", item.action or ""):width("fill")
            if props.color then button = button:color(props.color) end
            return button
        end)
    if props.onEntered then list = list:onEntered(props.onEntered) end
    if props.onExit then list = list:onExit(props.onExit) end
    if props.onDisplaced then list = list:onDisplaced(props.onDisplaced) end
    return list
end)
