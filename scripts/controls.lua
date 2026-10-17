-- Run: riced lua-demo --script scripts/controls.lua
local app = { enabled = false, volume = 25, query = "", choice = "one", status = "Ready" }

function app:view()
    return iced.column({
        iced.text("Native iced controls from Lua"):size(24),
        iced.checkbox("Enabled", self.enabled):on_toggle("enabled"),
        iced.toggler("Same state", self.enabled):on_toggle("enabled"),
        iced.text("Volume: " .. self.volume),
        iced.slider(0, 100, self.volume):step(1):on_change("volume"):on_release("released"),
        iced.text_input("Search", self.query):on_input("query"):on_submit("search"),
        iced.pick_list({ "one", "two" }, self.choice):on_select("choice"),
        iced.radio("One", "one", self.choice):on_select("choice"),
        iced.radio("Two", "two", self.choice):on_select("choice"),
        iced.text(self.status),
        iced.button("Reset", "reset"),
    }):spacing(12):width(size.fill)
end

function app:actions(events)
    if events.type ~= "action" then return end
    local key, value = events.key, events.value
    if key == "reset" then
        self.enabled, self.volume, self.query, self.choice = false, 25, "", "one"
        self.status = "Reset"
    elseif key == "search" then
        self.status = "Search: " .. self.query
    elseif key == "released" then
        self.status = "Volume set to " .. self.volume
    else
        self[key] = value
    end
end

return app
