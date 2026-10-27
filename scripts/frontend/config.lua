-- Preferences for the original shell flow. root.lua owns IPC and extra layers.
return {
    background = { disabled = false },
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
