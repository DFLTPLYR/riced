# riced

Wayland layer-shell daemon built with [`iced` 0.14](https://github.com/iced-rs/iced) + [`iced_exwlshell` 0.20](https://github.com/wayland-rs/iced_exwlshell) — per-output fullscreen backgrounds with global drag-selection, context menu, and top bars. Ported from a Quickshell `Background` + `selectionRect` QML singleton.

## About this project

Experiment in AI engineering: the code here is AI-generated from
developer-written prompts rather than hand-written. The developer guides,
reviews, and tests every change — prompts in, review before merge.

## Features

- **Layer shell daemon** (`daemon` + `exwlshell`) — one `Background` (`Layer::Background`, `Anchor::all`, `Size::FILL`) + one `Top` bar (`50px`, `Anchor::Top`, `exclusive_zone`) per output via `OutputInsert`
- **Global selectionRect** — `QtObject { startPoint, selecting, x/y/width/height, onSelectingChanged { if(!selecting) width=0 height=0 startPoint=null } }` → `SelectionRect` `src/app/app.rs:17` with `set_selecting` `150ms InOutQuad` fade, `to_global` `mapToGlobal` via `OutputInfo.logical_position` `src/app/app.rs:212`
- **Cross-monitor drag** — global `x/y/w/h` clipped per `panel.screen` `min(x+w,sx+sw)-max(x,sx)` `src/app/app.rs:267` + `Utils.intersects` `src/app/app.rs:225`, `Scope::All` on `SelectionTick` `src/app/app.rs:591`
- **Smooth input** — `hoverEnabled: selecting` via `AtomicBool SELECTING` `src/app/app.rs:105`, `CursorMoved` throttled at source `16ms ~60fps` `src/app/app.rs:107` + `16ms` in `update` `src/app/app.rs:490`, filtered to `Event::Mouse` only (drops `RedrawRequested` flood), `SelectionTick` drives `All` redraw; `canvas` `SelCanvas` `src/app/app.rs:52` single `fill/stroke` vs `container+stack` layout
- **Context menu** — `Right` `ButtonPressed` `src/app/app.rs:525` `contextMenu {x,y,open}` `src/app/app.rs:42`, clipped to screen, `Left` press closes

## Requirements

- Wayland compositor with `wlr-layer-shell` (Hyprland, Sway, Niri, etc.)
- `wayland-client` connection (`WAYLAND_DISPLAY`)
- `libxkbcommon` (`xkbcommon.pc` on `PKG_CONFIG_PATH`) — Nix provides it
- Rust `>=1.85` (edition `2024`), `cargo` with `tokio`, `wgpu`/`tiny-skia`, `canvas` features `Cargo.toml:7`

## Install & Run

```bash
# Nix (recommended, provides xkbcommon, wayland)
nix develop # or direnv
cargo run --release              # release ~3× smoother than debug (wgpu validation off)
RUST_LOG=warn cargo run --release # suppress info flood, keeps WARN tracker
# or
PKG_CONFIG_PATH=/nix/store/...-libxkbcommon-*/lib/pkgconfig cargo run --release

# debug (choppy, for logs)
cargo run
RUST_LOG=warn cargo run
```

`cargo run --release` parses `Settings` `src/main.rs:24` — `RUST_LOG=warn cargo run --release` is correct (`RUST_LOG=warn` is env, not arg).

## Usage

Left-drag on any `Background` (fullscreen) starts `selecting`:

```
select start Point { x, y } (local Point { x, y })  # src/app/app.rs:554
select end -> reset rect (fade 150ms)                # src/app/app.rs:567
```

- `Left` `MouseArea.anchors.fill parent` `src/app/app.rs:354` `propagateComposedEvents:true` → `selecting=true startPoint=mapToGlobal(mouse.x,mouse.y)` `src/app/app.rs:543`
- `CursorMoved` while `selecting` → `min/max` `src/app/app.rs:500` `width = max-min`
- `Left Released` → `selecting=false` → `width=0 height=0 startPoint=None` `src/app/app.rs:29` + `fade_rect` `150ms` `opacity 1-eased` `src/app/app.rs:251`
- `Right` → `contextMenu` at `x/y` `src/app/app.rs:525`, next `Left` closes

`Top` bar `src/app/layers/top.rs:14` is per-output `Top` layer, not selectable.

## Architecture

```
src/main.rs:10          daemon(Plots::new, namespace, update, view)
                        .subscription(Plots::subscription) // mouse throttled + shell
                        .settings(Settings { layer_settings: Active 1px placeholder })
                        // per-output Background+Top spawned via Plant::NewLayerShell on OutputInsert
src/app/mod.rs:1        app, events, layers
src/app/events.rs:13    Plant { Grow, Uproot(Id), Tend, Wayland(WayEvent), Graft(Id,Event), SelectionTick } #[to_layer_message(multi)]
src/app/app.rs:17       SelectionRect, ContextMenu, SelCanvas (canvas::Program), Plots { ids, tops/top_ids, backgrounds/background_ids, output_infos, last_cursor, selection_rect/fade, last_selection_tick }
src/app/layers/background.rs:12  Background::open(GlobalName) FILL Anchor::all
src/app/layers/top.rs:14        Top::open(GlobalName) fill_width(50) Anchor::Top
src/app/app.rs:99       subscription: listen_with(throttled_graft) + shell_events + time::every(16ms) while fading
src/app/app.rs:454      redraw_scope: SelectionTick|Button => All, CursorMoved => None
```

## Widgets (Lua)

Bar cells are Lua scripts in `~/.config/riced/widgets/` — each file
is one widget named by its stem, placed by name from `[[bar]]`
`widgets` entries. First run seeds clock plus commented
hello/stats/cpu/ram/gpu/workspaces/clinepass examples — add a
`[[bar]]` slot entry (or drag it in from the Settings pool) to use one.

```toml
# [[bar]] widgets entry: bare name inherits everything, tables
# override per instance.
widgets = [[{ name = "clock", size = 16.0 }]]
```

```lua
-- clock.lua: methods are bound to this returned app instance.
-- `defaults` declares interval/size/custom props (overridable per
-- placement in Settings); `self.props` carries resolved values.
local app = {
    defaults = {
        interval = 1.0,
        size = 13.0,
        props = { format = "%H:%M" },
    },
}
function app:view()
    return ui.row({ ui.icon("clock"), ui.text(os.date(self.props.format)) })
end
function app:popup()
    return { ui = ui.text(os.date("%A, %d %B %Y")), width = 300, height = 200 }
end
return app
```

- **Lifecycle**: scripts return an app table with `app:view()`;
  each placement gets an independent state (`self`). `app:view()`
  re-runs every effective `interval` (definition default or placement
  override); output changes repaint. `.lua` edits hot-reload,
  definition add/remove rescans rebuild states, errors log once per
  message. `self.props` carries resolved custom properties.
- **Shell**: string/table/math/os/io with native shell —
  `os.execute(cmd)` runs, `io.popen(cmd):read("*a")` captures stdout.
  No allowlist (`;` chains work; owner-accepted risk). `os.exit` /
  `os.remove` / `os.rename` and `require` stay blocked.
- **Services**: `system` (cpu/mem/gpu or nil), `theme` (live iced
  palette hex for `:color()`), `notifications` (queue snapshot,
  newest-first), `wayland` (`outputs`, native `workspaces` from
  `ext-workspace` and `toplevels` from `ext-foreign-toplevel-list`;
  empty where unsupported, no focus), plus `bar.output` (the
  rendering bar's connector, nil when unknown),
  republished before every due render/popup/action.
  `ui.*` (`text`, `icon` — full Lucide set, `row`, `column`,
  `button`, `progress`, `spinner`, `separator`).
  `ui.define(name, fn)` registers reusable builders called as `ui.name(props)`;
  components from `components/*.lua` (seeded `spacer`,
  `card`, `menu`); component edits rebuild all states like a
  widgets-dir rescan.
- **Clicks**: `app:popup()` toggles a menu (`text`/`width`/`height`/
  `items`/`ui`); else `app:on_press()` runs. Cell `ui.button`s call
  that app's `app:on_action(key)` directly; popup rows do the same.
- **Slow fetches**: return `ui.spinner()` first, keep cached data on
  the app instance, then show cached rows (see `clinepass.lua`).
- **List transitions**: top-level row/column `ui.button`s animate on
  add/remove (slide, keyed by action, animation-speed duration).
  Notification cards share the same machine, keyed by id. Label edits
  swap instantly; first paint settles with no animation. An optional
  `app:transitions()` method overrides add/remove/displaced per widget
  (QML `Transition` subset: `x`/`y`/`opacity` `{from, to}` + `duration`).
- **Notification center**: the `notifications` global (newest-first
  `{id, app, title, body, urgency, has_image}`) is republished before
  every view; `app:on_action` may return `{ dismiss = id }` or
  `{ invoke = { id, key } }` to act on the queue (see `notifycenter.lua`).

Full contract with shapes and edge cases lives on `WidgetDef` in
`src/config.rs` (the `/// Widgets:` doc block).

### Component-built animated lists

Registered components are called directly (`ui.card(props)`). Setters are repeatable;
properties are stored separately from methods.

```lua
function app:popup()
    return { width = 320, height = 400, ui = ui.scrollable(
        ui.listview(notifications)
            :id("center"):key("id"):pitch(32):spacing(4)
            :delegate(function(n)
                return ui.button(n.title, "dismiss:" .. n.id):width("fill")
            end)
            :onEntered({ x = { from = 200, to = 0 },
                         opacity = { from = 0, to = 1 }, duration = 250 })
            :onExit({ x = { to = -200 }, opacity = { to = 0 }, duration = 250 })
            :onDisplaced({ duration = 250 })
    ) }
end
```

List ids are unique within a widget or popup; item keys are unique string
or integer fields. Delegates run when Lua produces the tree, not on animation
frames. `pitch` is the estimated item size plus spacing along the list axis;
`:axis("horizontal")` selects a horizontal list. Exits paint as inert overlays
without reserving layout slots. The app's `app:transitions()` method is
supported for automatic button lists.

Other composition primitives are `ui.container(child)` (width, height,
padding, background, radius), `ui.scrollable(child)`, `ui.space()`, and
`ui.image(path)` (width and height). Card props may supply `padding`, `radius`,
and `background` to request a styled surface. Images accept file paths or
`file://` paths. Fades multiply primitive alpha; they are not offscreen
subtree/group compositing.

Component files execute separately with their filenames in errors. Changes
are detected from the complete file manifest and contents, including removals.
A replacement library is validated before use; an invalid edit keeps the last
working library. Notification queue changes refresh widgets and open center
popups immediately.

## Declarative Lua runtime: M0 demo

```bash
nix develop --command cargo run -- lua-demo
nix develop --command cargo run -- lua-demo --script ./scripts/main.lua
```

The tested software-renderer launch in this Nix environment supplies the
runtime Wayland libraries explicitly:

```bash
nix develop --command bash -c '
  export LD_LIBRARY_PATH="$(pkg-config --variable=libdir wayland-client):$(pkg-config --variable=libdir xkbcommon):${LD_LIBRARY_PATH:-}"
  ICED_BACKEND=tiny-skia cargo run -- lua-demo --script ./scripts/main.lua
'
```

`src/lua/mod.rs` hosts one Lua VM for this application. The embedded
`scripts/main.lua` returns an app table with an `app:view(window_id)` method;
an explicit script path, or `~/.config/riced/main.lua` when present, overrides
the embedded script. Changes to an override reload live. Invalid scripts and
runaway handlers become an error banner with a reload button, retaining the
last successful view.

The host uses protected Lua calls, a 64 MiB VM allocation limit, an instruction
budget per entry, and incremental GC on its idle tick. Each app load gets a
fresh environment with the shared `ui`/`iced` and `riced` API. Lua produces
description tables; Rust decodes them into owned IR, caches it by window and
version, and realizes Elements in iced's pure view function. Named host events
carry owned JSON payloads via mlua serialization. `riced.invalidate()` records
an effect; handlers also conservatively invalidate the cached view.

Bundled Lua sources live in `scripts/widgets/` and `scripts/components/`,
and seed installed copies on startup. Widget and notification modules return
app tables; the shell calls their methods through the registry. Global
`render()` scripts are rejected, and no global compatibility exports are generated.

Shared components are installed in `~/.config/riced/components/` (or
`$XDG_CONFIG_HOME/riced/components/`), alongside `widgets/`. Startup migrates
Lua files from the former `widgets/components/` directory, preserving any
existing root-level files. The library is loaded in filename order and hot-reloaded.
Unnumbered overrides (`define.lua`, `styled.lua`, `card.lua`, `menu.lua`)
replace their corresponding numbered seed files during loading; startup also
skips seeding those numbered copies when the override exists.

The styled builders accept props and support subsequent setter chaining:

```lua
ui.surface({ body = ui.text("Hello"), background = theme.surface,
             border = theme.outline, border_width = 1, radius = 8, padding = 10 })
ui.styled_button({ label = "Go", action = "go", background = theme.surface })
ui.styled_progress({ value = 0.5, color = theme.outline, background = theme.surface })
ui.styled_separator({ color = theme.outline, height = 2 })
```

`surface` forwards container sizing and styling, `styled_button` forwards
button sizing, padding, label color and resting background, and
`styled_progress` forwards sizing, fill color and track background.
`card` also accepts `border`, `border_width`, and `separator_color`;
`menu` accepts button `background` and `padding` alongside `color`.

### App-backed shell composables

The first migrated shell surface is the drag-selection rectangle:

```toml
[composable.selection_rect]
src = "selection_rect.lua"
```

Relative `src` paths resolve under `~/.config/riced/components/`; absolute
paths are also supported. The bundled file is seeded without overwriting
existing files. App-backed modules start with `-- riced:composable` so they
are excluded from automatic shared-builder loading, and return an app table:

```lua
-- riced:composable
local app = { defaults = { radius = 4, border_width = 1 } }

function app:view(props)
    return ui.container(ui.space())
        :width(props.width):height(props.height)
        :background(theme.surface)
        :border(theme.primary):border_width(props.border_width):radius(props.radius)
end

return app
```

`props` and `self.props` are the same resolved table: Lua `app.defaults`, then
optional `[composable.selection_rect.props]` scalar overrides, then host-owned
state. Rust supplies `width`, `height`, output-local `x`/`y`, `opacity`,
`selecting`, and an `output` geometry table. The host positions and fades the
returned tree, so the component should not apply `opacity` a second time.
The bundled appearance defaults (`radius`, `border_width`, `fill_alpha`) live
in Lua. Read `theme.primary` and other theme values inside `view` to follow
live theme changes.

Views are cached per output and re-evaluated when props or the theme change.
Source and shared-library edits reload automatically; failed reloads retain
the previous app and keep rendering it with current drag geometry. If no
working app exists, the native selection appearance is used. Entries use
the bounded Lua host (instruction budget and memory limit).

Context menus, popup/bar/notification frames, and interactive settings
components still use their current Rust paths; they are the next migration
stages. Existing `[composable.menu]` and `[composable.context_menu*]` scalar
settings remain active until those surfaces are migrated.

This is **M0**, the first runnable migration milestone. It reuses the existing
owned node decoder/realizer as an adapter. The shell still hosts separate
widget VMs; the shared application host has one VM. Dedicated `ui::ir`/`RealizeCtx`
with the borrowed `StateStore` belongs to M1; callback generations belong to
M2; task effects, user subscriptions, and layer-window control belong to M4.
The demo's static Lua tree does not register interactive closure callbacks.

## Notifications

D-Bus freedesktop server (mako replacement) plus internal events,
rendered per output — mouse output by default, pinnable by name:

```toml
[notifications]
enabled = true
output = "mouse"  # or "DP-1" to pin
position = "top-right"
timeout_ms = 5000
```

- **Sources**: any D-Bus client (`notify-send` works once riced owns
  the bus name; yields silently to mako/dunst if one runs), plus
  internal events — config/widgets parse errors, template failures,
  theme-regen results (critical ones persist until clicked).
- **Lua layout**: `widgets/notifications.lua` defines
  `app:view(n)` over `{ id, app, title, body, icon, urgency, actions,
  has_image }`
  (`actions` is a 1-based array of `{ key, label }` tables, empty when
  the sender offers none) with the full `ui.*` set; edits re-render
  visible cards on save, and deleting the file restores the built-in
  card. Policy (timeout, position, cap) stays in TOML.
- **Behavior**: one layer window per showing output (closed when
  empty), spanning the full output height like a listview — the whole
  stack shows newest-first and scrolls, no cap (`max_visible` in old
  configs is ignored). Click a card to dismiss, expiries swept on a
  250ms tick that only runs while the queue is non-empty. Actions are
  live: the built-in card renders one button per action, and button
  clicks emit `ActionInvoked` back over D-Bus (then dismiss with
  reason "invoked"); capability `actions` is claimed alongside `body`.
  App images render too: `image-data` pixbufs first, then the
  `image-path` hint, then `app_icon` as a file path — a 36px thumbnail
  left of the card body (themed icon names still render text-only).
  Scripts see `n.has_image` to adapt layout.

## Configuration

`src/main.rs:24` `LayerShellSettings`:

```rust
anchor: Anchor::Top, size: LayerSize::fill_width(1), exclusive_zone: 0, start_mode: Active // daemon tiny
// per-output Background: Anchor::all Size::FILL Background (src/app/layers/background.rs:12)
// per-output Top: Anchor::Top Size::fill_width(50) exclusive_zone 50 Top (src/app/layers/top.rs:22)
```

`exclusive_zone: i32` not `LayerSize` `src/settings.rs:92`.

## Theme

Global theme lives in `src/theme.rs` — every color/style helper reads one
`ActiveTheme`, so a palette tweak repaints everywhere on the next frame.

Theme files use the exact reshell schema
(`dotfiles/.config/quickshell/reshell/core/data/themes/*.json`, vendored
under `themes/`): `{ "dark": {...16 colors...}, "light": {...} }` plus an
ignored `terminal` block. Selected in `config.toml` (mirrors reshell
`general.theme` + `general.darkmode`):

```toml
[theme]
name = "gruvbox" # ~/.config/riced/theme/{name}.json (user file wins, else vendored copy)
darkmode = true  # picks the dark vs light variant
variant = "content" # Material You scheme variant for generate-theme
```

First run seeds `~/.config/riced/theme/` with all 11 vendored themes so
they are editable; edits hot-reload on the 500ms `ConfigTick` (same as
`config.toml`). A Theme page in the Settings panel lists themes + a
Dark/Light toggle, persisted via `ConfigPatch::ThemeName/Darkmode`.

## Dynamic theme (colorgen)

`src/colorgen.rs` ports `sys/src/colorgen.rs` (`ColorGen.generate`):
`riced generate-theme [--variant V] [--set] [--templates DIR]` rasterizes
the wallpaper set with the same math as `Background::wallpaper_views`
(per-output crops, ascending `z`, stitched side-by-side), extracts the seed
color, and writes the M3 scheme as `dynamic.json` — same
`{light, dark}` keys and terminal mapping as sys, so files are drop-in
reshell themes. `dynamic` is never vendored (it depends on the wallpapers);
`--set` selects it so the daemon hot-reloads. `--templates DIR` overrides
the templates dir, `--no-templates` skips rendering.

Default templates ship in `templates/` (copied from reshell `core/theme/`:
kitty, hypr, gtk, helix, rofi, btop, discord, nvim, … plus its `config.toml`).
First run seeds them to `~/.config/riced/templates/` (missing files only,
never overwrites), and that copy is what renders by default — edit it
freely. `[theme] templates_dir` overrides the dir, `"off"` disables.

Switching themes re-renders templates too (sys `change_theme`): picking a
theme in Settings (or `riced apply-templates [--templates DIR] [--name X]`)
passes the stored theme object through the same find-and-replace off the
update thread, reporting per-template errors without freezing the shell.

`dynamic.json` also regenerates itself without polling: dropping a dragged
wallpaper, adding/removing/scaling one, or hand-editing
`[background.image]` arms a one-shot 2s countdown (per-move patches stay
silent, so drags never clog the events), and closing the last Settings
panel fires immediately. Generation runs on a blocking worker from the
live output rects and repaints all outputs when done; runs only while
the selected theme is `dynamic`.

## Development

```bash
cargo check   # with PKG_CONFIG_PATH for xkbcommon
cargo fmt
cargo clippy
```

Flake `flake.nix` provides devShell. `Session.vim` for vim.

## Troubleshooting

- `xkbcommon.pc not found` → `PKG_CONFIG_PATH=/nix/store/...-libxkbcommon-*/lib/pkgconfig`
- `TrySendError Full` `WARN tracker` → `CursorMoved` flood, now throttled `src/app/app.rs:107` + ` Event::Mouse` filter `src/app/app.rs:189`
- `left lines on shrink` → was `canvas stroke 0.5px` + `stack` child swap `Space` `src/app/app.rs:279`; now `container Rectangle` `background+border` `src/app/app.rs:285` always `Fill`
- `startPoint random` → `last_cursor` stale when `!selecting`; now `LAST_CURSOR_GLOBAL` `src/app/app.rs:49` updated at source even when `None`
- Entire app laggy → `dev` `wgpu` validation + `println!` per drag + `Scope::All` per pixel; `release` + `RUST_LOG=warn` + `60fps` throttle fixes

## License

MIT (or as `Cargo.toml` specifies)
