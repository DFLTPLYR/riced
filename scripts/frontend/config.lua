-- Preferences for the Lua-owned shell. Bars and widgets are declared by
-- root.lua/layers, so no native bar host is enabled.
return {
    -- The background surface is declared by layers/background.lua; the
    -- wallpaper and desktop-widget leaves are still host-filled.
    background = { disabled = true },
    bar_host = false,
    theme = { name = "gruvbox", mode = "dark", templates_dir = "off" },
}
