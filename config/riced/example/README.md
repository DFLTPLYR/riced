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
`layers/`, `plugins/`, and `settings/` contain the copied customizations.
The previous `apps/background` migration sample is no longer included.

The config retains personal `DP-1` placement and wallpaper paths; adjust them
for your machine. This is a verbatim installed snapshot, not the latest bundled
frontend: its handlers do not include the newer per-output bar targeting or
notification action-key fixes in `scripts/frontend/`.

Seeded libraries under `components/`, `widgets/`, `shaders/`, `theme/`, and
`templates/` are included in this snapshot so its customizations are preserved.
Runtime locks and initialization markers are not committed.
