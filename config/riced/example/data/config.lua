-- riced configuration. Settings rewrites this file; edits reload live.
return {
    desktop = {
        { output = "DP-1", widget = "usage", x = 1635.03125, y = 930.528076171875 },
    },
    background = {
        disabled = true,
        mode = "per_monitor",
        monitors = {
            ["DP-1"] = { path = "/home/dfltplyr/Pictures/Wallpaper/clouds.png" },
        },
    },
    theme = {
        icon_theme = "breeze-dark",
        mode = "dark",
        name = "rosepine",
        templates_dir = "off",
    },
    animation = { speed = "fast" },
    interaction = { hold = "medium" },
    notifications = { output = "mouse", source = "notifications.lua", timeout_ms = 5000 },
    bar_host = false,
}