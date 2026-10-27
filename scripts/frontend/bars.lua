-- Bar data, owned by root.lua. `riced ipc add/remove` and Settings → Panel
-- edit this file; the next `view` declares the matching layers. Each bar has
-- an ordered slot list, so any number of regions and widgets is allowed.
return {
    bars = {
        {
            edge = "top",
            thickness = 34,
            slots = {
                { align = "start", widgets = { "workspaces" } },
                { align = "center", widgets = { "clock" } },
                { align = "end", widgets = { "systemtray", "notifycenter", "system" } },
            },
        },
    },
}
