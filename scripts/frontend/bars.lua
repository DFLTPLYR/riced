-- Bar data, owned by root.lua. `riced ipc add/remove` edits this file; the
-- next `view` declares the matching layers. Widget names are files in
-- `widgets/`.
return {
    bars = {
        {
            edge = "top",
            thickness = 34,
            widgets = {
                start = { "workspaces" },
                center = { "clock" },
                ["end"] = { "systemtray", "notifycenter", "system" },
            },
        },
    },
}
