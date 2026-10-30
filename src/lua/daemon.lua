-- The builder records callbacks; the native runner owns the event loop.
return function(new, namespace, update, view)
    for name, callback in pairs({ new = new, namespace = namespace, update = update, view = view }) do
        assert(type(callback) == "function", "daemon." .. name .. " must be a function")
    end
    assert(new and namespace and update and view, "daemon requires new, namespace, update and view")
    local callbacks = { namespace = namespace }
    local options = {}
    local builder = {}
    for _, name in ipairs({ "title", "subscription", "theme", "style", "redraw_scope" }) do
        builder[name] = function(self, callback)
            assert(type(callback) == "function", "daemon." .. name .. " must be a function")
            if name == "subscription" then
                local original = callback
                callback = function(state, facts)
                    local selected = original(state, facts)
                    assert(type(selected) == "table", "daemon.subscription must return an array")
                    local allowed = { input=true, wayland=true, config=true, ipc=true, tray=true,
                        desktop=true, apps=true, settings=true, animation=true,
                        notification_deadlines=true, overlays=true, notifications=true,
                        selection=true, widgets=true }
                    local seen = {}
                    for key, source in pairs(selected) do
                        assert(type(key) == "number" and key >= 1 and key <= #selected and key % 1 == 0,
                            "daemon.subscription must return a dense array")
                        assert(allowed[source], "unknown daemon subscription: " .. tostring(source))
                        assert(not seen[source], "duplicate daemon subscription: " .. source)
                        seen[source] = true
                    end
                    return selected
                end
            end
            callbacks[name] = callback
            return self
        end
    end
    function builder:settings(value)
        assert(type(value) == "table", "daemon.settings must be a table")
        local allowed = { id = true, antialiasing = true, default_text_size = true,
            keep_compositor_alive = true, layer_settings = true, tracing = true }
        for key in pairs(value) do assert(allowed[key], "unknown daemon setting: " .. key) end
        options = {
            tracing = value.tracing,
            connection = { id = value.id, antialiasing = value.antialiasing,
                default_text_size = value.default_text_size,
                keep_compositor_alive = value.keep_compositor_alive },
            settings = value.layer_settings,
        }
        return self
    end
    function builder:run()
        assert(not self.started, "daemon.run may only be called once")
        self.started = true
        local app = new()
        assert(type(app) == "table", "daemon.new must return a state table")
        app.view = view
        app.actions = update
        app.on_tick = function(self) return update(self, { type = "tick" }) end
        app._daemon = callbacks
        return riced.root(app, options)
    end
    return builder
end
