-- Preferences for the original shell flow. root.lua owns IPC and composes
-- the `layers/` modules for any native host disabled here.
return {
    -- Native desktop host. Set `disabled = true` to draw
    -- layers/background.lua instead (wallpaper and desktop widgets are still
    -- host-filled through its native slots).
    background = { disabled = false },
    -- Native bar host. Set false to draw bars from root.lua/layers/bar.lua.
    bar_host = true,
    bar = {
        {
            anchor = "top", thickness = 34,
            slots = {
                { align = "start", widgets = { "workspaces" } },
                { align = "center", widgets = { "clock" } },
                { align = "end", widgets = { "systemtray", "notifycenter", "system" } },
            },
        },
    },
    theme = { name = "gruvbox", mode = "dark", templates_dir = "off" },
}
