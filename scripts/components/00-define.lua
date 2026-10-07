-- Shared pure builders return declarative descriptions, not live Elements.
-- Register: ui.define("stat", function(props) ... return ui.row(...) end)
-- Call: ui.stat({icon="cpu", value="42%"}):width("fill")
ui.define("spacer", function(props)
    props = props or {}
    return ui.space():height(props.h or 8)
end)
