# riced

A Lua-configured Wayland desktop shell, rendered with Rust and iced. The bundled
frontend declares backgrounds, bars, notifications, and the Settings window from
one shared Lua state across all outputs.

## About this project

An experiment in AI engineering. Code is AI-generated from developer-written
prompts; the developer guides, reviews, and tests changes.

## Run

You need a Wayland compositor with layer-shell support, such as Hyprland, Sway,
or Niri, plus a current stable Rust toolchain. The Nix development shell supplies
the native build libraries, including Wayland and libxkbcommon.

```sh
nix develop
cargo run --release
```

Use release builds for normal desktop use. `RUST_LOG=warn cargo run --release`
reduces logging. Without Nix, install the native libraries and make their
pkg-config files available; use a current stable Rust toolchain.

Plain `riced` installs the bundled frontend into `$XDG_CONFIG_HOME/riced`
(normally `~/.config/riced`) when no configuration exists. It then starts the
Wayland daemon.

```sh
riced init             # install without starting Wayland
riced init --repair    # restore missing defaults; never replace existing files
riced -p /path/to/project
riced -c example       # from this repo: config/riced/example
```

`-p` selects a project directory. `-c NAME` searches `./config/riced/NAME`, then
`$XDG_CONFIG_HOME/riced/NAME`. Explicit projects do not populate the default XDG
project. The example is a personal frontend snapshot, not a portable default;
read [its setup notes](config/riced/example/README.md) first.

## Project files

```text
shell.lua                       shell entry point and startup options
data/
  config.lua                    preferences saved by Settings
  state.lua                     bundled IPC handlers
  bars/                         one Lua data file per bar
  plugins/                      Lua plugin modules
  theme/                        named theme JSON files
  templates/                    external-app theme templates
  state/                        initialization markers and locks
components/
  *.lua                         shared UI builders
  layers/                       background, bar, notification surfaces
  settings/                     Settings window and page layouts
  shaders/                      WGSL fragment files
composables/                    source-backed menu, selection, notification modules
widgets/                        bar and desktop widget scripts
apps/                           optional standalone Lua apps
extensions/                     optional project-owned Rust extension crates
```

Lua owns layout and interaction state. Rust owns rendering, native surfaces,
services, validation, and background jobs. The bundled `shell.lua` uses
`riced.daemon(...)`; standalone app tables remain supported.

Preferences belong in `data/config.lua`, not `shell.lua`. Settings rewrites the
preferences as a plain Lua data table without replacing frontend source files.
For example:

```lua
return {
    bar_host = false,
    background = { disabled = true }, -- shell.lua declares the background
    theme = { name = "gruvbox", mode = "dark", templates_dir = "off" },
}
```

The disabled flags turn off the legacy native surface hosts, not wallpaper
baking or embedded desktop widgets. Bars in the bundled frontend use
`data/bars/*.lua`; the legacy `bar` preference list is a separate host.

## Settings and bars

```sh
riced ipc settings
riced ipc add --anchor bottom --output DP-1 --thickness 34
riced ipc remove --anchor bottom --output DP-1
```

These commands are registered by `data/state.lua`; custom projects must register
their own handlers. IPC is session-wide, even when the client selects a project.
CLI success means the request was queued, not that its handler completed.

Omit `--output` for an all-output bar. Those use `data/bars/<edge>.lua`.
Targeted bars use `<edge>-output-<hex-encoded-name>.lua` and keep the original
connector name in `output`. Removing a targeted bar never removes an all-output
bar. Disconnected targets keep their files for reconnect.

**Settings → Panel** edits the same files, including their slots and widgets.
Add buttons create all-output bars; IPC can create output-specific bars.
Each embedded widget keeps its own Lua state, services, and action handler.

## Editing and upgrades

- Edit `shell.lua` and `components/layers/` to change shell composition.
- Edit `widgets/` for widget behavior and `components/*.lua` for shared builders.
- Edit `components/settings/` for Settings layouts. Invalid page copies fall
  back to bundled sources.
- Lua source edits reload automatically. Failed app replacements retain the
  last working runtime; successful reloads reset its in-memory state.
- Startup settings need a restart. Switching between a daemon entry and a plain
  app entry also needs a restart.

Startup and repair do not replace existing Lua files with new bundled versions.
Back up your project and merge changes from `scripts/frontend/`,
`scripts/settings/`, and the other bundled libraries explicitly. Repair restores
missing frontend defaults; it is not a reset or an automatic source upgrade.

Older project directories are moved into the layout above at startup. If old
and new paths contain different files, migration stops and keeps both versions;
resolve the reported conflict before restarting. Initialization history moves
with the files, so ordinary startup does not restore deliberately deleted libraries.

Legacy `root.lua` remains an entry fallback. A top-level `config.lua` is imported
into `data/config.lua` when the latter is absent, keeping the original. Legacy
`config.toml` is converted once and retained as `config.toml.migrated`; edit Lua
after conversion, not the retired TOML file.

## Wallpapers and themes

Open **Settings → Wallpaper**, edit the preview, then press **Apply** to publish
it. **Discard** restores the applied recipe; closing Settings does not apply a
draft. Per Monitor and Free Form modes retain separate recipes when switching.

Per-monitor assignments use connector names and survive reconnects. Available
fits are **Fit** (whole image with black margins), **Expand** (fill and crop),
**Stretch**, and **Center** (native pixels, cropped if needed).

Apply bakes lossless tiles at each output's native resolution from the source
pixels, not the small preview. Originals are never rewritten. Derived tiles and
their manifest live under `$XDG_CACHE_HOME/riced/wallpapers`. Failed jobs retain
the previous desktop. If config changes during Apply, it rejects publication;
reload before retrying. This is an optimistic check, not a lock against editors
racing the final file replacement.

Named themes live in `data/theme/` and reload automatically. Lua reads the
applied palette with `iced.palette()` and applies explicit overrides by returning
`iced.palette({...})` from a handler. See the
[palette API](docs/lua-api.md#icedpalette-built-in-not-a-widget).

```sh
riced generate-theme --set           # generate dynamic.json and select it
riced generate-theme --no-templates  # skip external-app templates
riced apply-templates --name gruvbox
```

The bundled `shell.lua` handles `wallpaper_applied` and explicitly requests
generation only when `event.theme.name == "dynamic"`. Change that Lua branch
to change the policy; Rust does not infer it from the selected theme. The native
worker retains the applied-pixel sampling, publication, and template handling.
Startup, output changes, and palette selection do not request generation.
Templates live in
`data/templates/`; `theme.templates_dir = "off"` disables them. Palette overrides
alone do not run template hooks. Templates can write external files and run
hooks: use trusted templates only.

## Notifications

Riced can own the freedesktop notification D-Bus service. If another notification
server owns the name, riced continues without replacing it. Lua declares the
notification layer; removing that layer hides presentation without disabling
the service.

Popup expiry hides the popup but retains the notification-center record.
History is session-only, capped at 256 records and 32 MiB of image bytes counted
per record. Oldest closed records go first, then oldest live records expire.
Snapshots, exit animations, and renderer caches can retain pixels longer; these
limits are not a cap on total process memory.

Image ingestion allows two workers, 8 MiB inputs, and raster dimensions up to
1024×1024, with a 16 MiB decoder allocation budget (best-effort inside codecs).
Invalid or excess image work falls back to text. SVGs render within 96px without
nested/linked images; raw pixel hints are limited to 512×512.

Return notification commands from handlers; calling a constructor alone does
nothing. See [notification commands](docs/lua-api.md#notification-scenes-widgets-popups-callbacks).

## Documentation

| Guide | Use it for |
| --- | --- |
| [Framework](docs/framework.md) | Daemon builder, app lifecycle, layers, windows, managed tasks, popups |
| [Lua API](docs/lua-api.md) | Widget constructors, setters, events, services, animations, shaders |
| [Persistence](docs/persistence.md) | `riced.file`, save conflicts, cancellation, file watching |
| [Extensions](docs/extensions.md) | Rust service crates, generated shells, supervised worker protocol |
| [Architecture](ARCHITECTURE.md) | Rust module ownership, data flow, validation |
| [Examples](examples/README.md) | Runnable standalone apps and extension examples |

The app, persistence, and extension APIs are experimental. Local Lua apps have
filesystem and shell access; execution budgets do not make untrusted scripts safe.

## Development

```sh
nix develop --command cargo test --workspace --locked
just check  # formatting, workspace tests, Clippy, whitespace checks
```

`just build` builds release and installs `~/.local/bin/riced`. `just install`
also stops the running daemon and starts the new binary; do not use it merely
to validate a change.

For the plain-window Lua demo:

```sh
nix develop --command cargo run -- lua-demo --script scripts/controls.lua
```

GPU shaders require the wgpu backend. Software rendering keeps their layout but
does not draw shader pixels. See [shader behavior](docs/lua-api.md#shaders).

## Troubleshooting

- **`xkbcommon.pc` not found:** use `nix develop` or fix `PKG_CONFIG_PATH`.
- **Cannot connect to Wayland:** run in a Wayland session with `WAYLAND_DISPLAY`.
- **Edits did not update bundled scripts:** startup preserves existing files;
  merge the new source explicitly.
- **A Lua edit fails:** inspect the daemon log; invalid replacements keep the
  last working app, but an invalid startup entry can prevent startup.
- **Slow debug rendering:** use a release build before investigating performance.
