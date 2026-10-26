-- Run from this repository using the commands in README.md beside this file.
-- Paths below are relative to examples/, not the working directory.
--
-- This mirrors the shell's configuration surface: a framework bar and cards,
-- the palette/theme/toplevel/workspace services, and the shell-level theme,
-- animation, interaction, and notification settings. Wallpaper and desktop
-- widget placements are machine-specific (absolute image paths, connector
-- names), so they stay commented out below.
return {
    extensions = {
        -- Compiled-in theme generation, registered from this project.
        { name = "theme_gen", path = "../extensions/theme-gen" },
        -- Compiled-in Wayland toplevel list (a second Wayland connection).
        { name = "wayland_toplevels", path = "../extensions/wayland-toplevels" },
        -- Compiled-in Wayland workspace list (a third Wayland connection).
        { name = "wayland_workspaces", path = "../extensions/wayland-workspaces" },
        { name = "palette_worker", path = "../extensions/palette-worker", process = {
            argv = { "../target/debug/riced-extension-palette-worker" },
            service = "palette",
            commands = { "generate" },
            timeout_ms = 30000,
        } },
        -- Project-owned process extension: `riced extension build hello`.
        { name = "hello", path = "extensions/hello", process = {
            argv = { "../target/debug/riced-extension-hello" },
            service = "hello",
            commands = { "greet" },
            timeout_ms = 5000,
        } },
    },
    apps = {
        { name = "bar", props = { height = 32 } },
        { name = "palette" },
        { name = "theme_gen" },
        { name = "editors" },
        { name = "hello" },
        { name = "toplevels" },
        { name = "workspaces" },
        { name = "popup" },
    },
    theme = {
        -- Built-in themes need no generated `dynamic.json`; `riced
        -- generate-theme` writes that when you want a wallpaper-derived one.
        name = "gruvbox",
        mode = "dark",
    },
    animation = { speed = "medium" },
    interaction = { hold = "fast" },
    notifications = {
        -- "mouse" follows the pointer; pin an output name like "DP-1" instead.
        output = "mouse",
        -- Relative to components/; this is the bundled scene.
        source = "notifications.lua",
        timeout_ms = 5000,
    },
    -- Wallpaper. Relative paths resolve from the launch directory, so use
    -- absolute paths, or per-monitor entries keyed by connector name:
    -- background = {
    --     images = {
    --         { path = "/path/to/wallpaper.png", position = { 0.0, 0.0 }, scale = 1.0 },
    --     },
    --     -- mode = "per_monitor",
    --     -- monitors = { ["DP-1"] = { path = "/path/to/wide.png", fit = "stretch" } },
    -- },
    -- Desktop widget placements are per-connector and per-widget:
    -- desktop = {
    --     { output = "DP-1", widget = "clock", x = 100.0, y = 100.0 },
    -- },
    -- Legacy bars remain supported; do not enable both on the same edge.
    bar = {},
}
