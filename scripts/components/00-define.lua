-- Shared pure builders return declarative descriptions, not live Elements.
-- Register: iced.define("stat", function(props) ... return iced.row(...) end)
-- Call: iced.stat({icon="cpu", value="42%"}):width(size.fill)
iced.define("spacer", function(props)
    props = props or {}
    return iced.space():height(props.h or 8)
end)
