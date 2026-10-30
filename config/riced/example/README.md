# Personal frontend snapshot

This example copies `~/.config/riced`, including its config values and Lua
customizations. From the repository root, after rebuilding riced:

```sh
riced -p ./config/riced/example
# Equivalent shorthand:
riced -c example
```

Run only one riced daemon per session; IPC is session-wide. The selected project
keeps its files separate from your normal config.

`shell.lua` declares backgrounds, notifications, and bars from `data/bars/`.
This snapshot still stores customizations in legacy `layers/`, `plugins/`, and
`settings/` directories. Startup moves them to `components/layers/`,
`data/plugins/`, and `components/settings/`; it also moves `state.lua` to
`data/state.lua`. This uses a plain app entry, not the bundled daemon builder.
Use a separate copy if you do not want startup to move files in this checkout.

The config retains personal `DP-1` placement and wallpaper paths; adjust them
for your machine. This is a verbatim installed snapshot, not the latest bundled
frontend: its handlers do not include the newer per-output bar targeting or
notification action-key fixes in `scripts/frontend/`.

Seeded libraries under `components/`, `widgets/`, `shaders/`, `theme/`, and
`templates/` are included. Startup moves the last three to `components/shaders/`,
`data/theme/`, and `data/templates/`, and reusable composables to `composables/`.
Runtime locks and initialization markers are not committed.
