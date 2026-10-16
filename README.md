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

## Configuration format

`~/.config/riced/config.toml` uses kebab-case keys. Component appearance belongs
in Lua; TOML selects sources and describes shell placement. Saves omit default
bar geometry and native image dimensions, and preserve widget inheritance.
An explicit widget override stays explicit, even if it currently matches a Lua
default. Bare widget names follow their Lua defaults as those defaults change.

Compact slots keep alignment next to their ordered widgets; their count is inferred:

```toml
[[bar]]
output = "DP-1"
anchor = "top"
thickness = 34
slot-spacing = 11
slots = [
  { align = "start", widgets = [{ name = "workspaces", size = 8 }, "stats"] },
  { align = "center", widgets = ["clock", "notifycenter"] },
  { align = "end", widgets = [] },
]
```

Expanded slots are the same schema, useful for larger configurations:

```toml
[[bar]]
output = "DP-1"
thickness = 34

[[bar.slots]]
align = "start"
widgets = ["workspaces", { name = "stats", size = 16 }]

[[bar.slots]]
align = "center"
widgets = ["clock"]
```

Slots default to center alignment and empty widgets. An omitted `slots` field
creates one empty slot. Empty slots in an explicit list are retained.
Settings writes compact slot arrays, including inline widget overrides.

Margins and radii accept uniform numbers or named sides/corners:

```toml
[[bar]]
floating = true
margin = 8
radius = 10

[[bar]]
margin = { top = 8, right = 12, bottom = 8, left = 12 }
radius = { top-left = 10, top-right = 10, bottom-left = 4, bottom-right = 4 }
```

Unspecified sides/corners default to zero. Other bar defaults are `anchor = "top"`,
`output = ""` (all outputs), `length = 100`, `thickness = 50`, `slot-padding = 0`,
`slot-spacing = 4`, `opacity = 1`, and `floating = false`.

Wallpaper assignments use `[background.monitors.<output>]`; ordered free-form
layers use `[[background.images]]` with `position = [x, y]`. Both recipes are
retained when switching modes. Image defaults are position `[0, 0]`, `z = 0`,
`scale = 1`, and native dimensions (`width = 0`, `height = 0`).
Theme selection uses `mode = "dark"` or `"light"`, with optional `templates-dir`.
Notification timeout is spelled `timeout-ms`.

Old flat layouts migrate once at file load, preserving the original as
`config.toml.pre-structured`. Conflicting old/new fields fail validation instead
of silently overriding one another. Saved configs use only the new structure.

## Widgets (Lua)

Bar cells are Lua scripts in `~/.config/riced/widgets/` — each file
is one widget named by its stem, placed by name from `[[bar]]`
`widgets` entries. First run seeds clock plus commented
hello/stats/cpu/ram/gpu/workspaces/clinepass examples — add a
`[[bar]]` slot entry (or drag it in from the Settings pool) to use one.

```toml
# [[bar]] widgets entry: bare name inherits everything, tables
# override per instance.
slots = [{ widgets = [{ name = "clock", size = 16.0 }] }]
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
    return iced.row({ iced.icon("clock"), iced.text(os.date(self.props.format)) })
end
function app:popup()
    return { ui = iced.text(os.date("%A, %d %B %Y")), width = 300, height = 200 }
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
  `iced.*` (`text`, `icon` — full Lucide set, `row`, `column`,
   `button`, `progress`, `spinner`, `separator`, plus the native controls below).
  `iced.define(name, fn)` registers reusable builders called as `iced.name(props)`;
  components from `components/*.lua` (seeded `spacer`,
  `card`, `menu`); component edits rebuild all states like a
  widgets-dir rescan.
- **Clicks**: `app:popup()` toggles a menu (`text`/`width`/`height`/
  `items`/`ui`); else `app:on_press()` runs. Cell `iced.button`s call
  that app's `app:on_action(key)` directly; popup rows do the same.
- **Slow fetches**: return `iced.spinner()` first, keep cached data on
  the app instance, then show cached rows (see `clinepass.lua`).
- **List transitions**: top-level row/column `iced.button`s animate on
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

Registered components are called directly (`iced.card(props)`). Setters are repeatable;
properties are stored separately from methods.

```lua
function app:popup()
    return { width = 320, height = 400, ui = iced.scrollable(
        iced.listview(notifications)
            :id("center"):key("id"):pitch(32):spacing(4)
            :delegate(function(n)
                return iced.button(n.title, "dismiss:" .. n.id):width(size.fill)
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

Auto-sizing is the default for every UI primitive: omitting width or height uses
iced's native content-derived sizing on that axis. Containers, rows, columns,
scrollables, and lists inherit their children's sizing hints; images and spaces
size to their own pixels; text and buttons have always sized to their content.
Use `size.fill`, `size.shrink`, or `size.auto` to override the default
independently on either axis (`size.auto` is an alias for `size.shrink`):

```lua
iced.text("Hello"):width(size.fill):height(size.shrink)
```

The strict `size` table contains canonical string aliases; unknown keys such
as `size.fil` raise at the call site. Explicit pixel sizes and the plain
strings `"fill"`, `"shrink"`, and `"auto"` remain supported. For example,
`iced.container(iced.column({iced.text("Hello"), iced.text("World")})):padding(12)`
needs no dimensions to fit its content.

For `app:popup()`, omitting the returned table's `height` fits the native popup
window to the renderer-measured content, including padding, wrapped text, and
menu rows. The height updates when content changes and is bounded by the output.
An inner `:height("fill")` fills that fitted window; it does not force a window
height. Return `height = 400` alongside `width` and `ui` to request a fixed popup
height. Popup width retains its default of 280 unless explicitly supplied.
Measured heights are reused across reopenings through a bounded 32-entry cache.
Content, width, text size, and output-height changes invalidate the measurement;
trees with image nodes are remeasured because their files can change size.

Use `iced.scrollable(content):height(320):scrollbar("hidden")` to hide the
scrollbar while retaining wheel/touchpad scrolling. `:scrollbar("visible")`
restores the default bar. These are scrollable-only, case-insensitive values;
unknown names and non-string values raise an error. Constrain the viewport's
height so taller content can scroll.

Other composition primitives are `iced.container(child)` (width, height,
padding, background, radius), `iced.scrollable(child)`, `iced.space()`, and
`iced.image(path)` (width and height). Text accepts `:wrap("none" | "word" |
"glyph" | "word-or-glyph")` for each `text::Wrapping` strategy (default is
word wrapping, matching iced). Since iced has no native ellipsis, text also
accepts `:elide(n)` to keep at most `n` characters including the "…" mark,
with `:elide_mode("tail" | "head" | "middle")` (default `"tail"`):
`iced.text(title):wrap("none"):elide(24)`. Card props may supply `padding`, `radius`,
and `background` to request a styled surface. Images accept file paths or
`file://` paths. Fades multiply primitive alpha; they are not offscreen
subtree/group compositing.

Component files execute separately with their filenames in errors. Changes
are detected from the complete file manifest and contents, including removals.
A replacement library is validated before use; an invalid edit keeps the last
working library. Notification queue changes refresh widgets and open center
popups immediately.

## Declarative Lua runtime: M0 demo

### Native controls and Rust component structs

`iced.*` is the constructor namespace; there is no `ui` global.
Popup return tables still use the
`ui` field, for example `{ ui = iced.text("Popup"), width = 300 }`.

| Constructor | Interaction setter | Payload | Rust description |
| --- | --- | --- | --- |
| `iced.checkbox(label, checked)` | `:on_toggle(key)` | boolean | `CheckboxNode` |
| `iced.toggler(label, enabled)` | `:on_toggle(key)` | boolean | `TogglerNode` |
| `iced.slider(min, max, value)` | `:on_change(key)` | number | `SliderNode` |
| `iced.text_input(placeholder, value)` | `:on_input(key)` | string | `TextInputNode` |
| `iced.pick_list(options, selected)` | `:on_select(key)` | string | `PickListNode` |
| `iced.radio(label, value, selected)` | `:on_select(key)` | string | `RadioNode` |

All accept `:width(...)`; the other supported setters are listed in
`iced.components.<name>.setters`. Slider supports `:step(number)`, pixel
`:height(number)`, and `:on_release(key)`. Text input supports
`:secure(boolean)` and `:on_submit(key)`. Submit, release, and button actions
send only the key. Controls without an interaction key stay inert.
Slider ranges must be finite with `min < max` and a positive step. Values
are clamped to the range. Pick-list options are unique strings; selection
is either nil or one of those options. Radio values and selections are strings.

State is owned by Lua and fed back into each view. Widget placements and the
plain-window demo dispatch changes to `app:on_action(key, value)`; existing
one-argument handlers continue working. Popup changes refresh the content
without dismissing the popup. Keyboard input requires a keyboard-capable host;
passive bar/background/notification surfaces do not acquire keyboard focus.
Notification and built-in context-menu actions keep their key-only protocols.

```lua
local app = { volume = 25 }
function app:view()
    return iced.slider(0, 100, self.volume):on_change("volume")
end
function app:on_action(key, value)
    if key == "volume" then self.volume = value end
end
return app
```

`src/ui/components.rs` declares each named Rust description and its Lua schema
together, including existing `TextNode`, `ContainerNode`, `RowNode`, and the
other primitives. Structs implement `mlua::FromLua` and `mlua::IntoLua` in a
host with the iced namespace installed. They own their data and can survive
the source VM or be exported into another VM. `WidgetNode` is the common IR.
`iced.components.text.rust_type` reports `"TextNode"`, and
`iced.to_table(node)` exports a normalized, chainable description. Listviews
export resolved keyed nodes with a fresh pure delegate; the original delegate
and data-source functions are not retained in the Rust description.

Try all six controls with:

```bash
riced lua-demo --script scripts/controls.lua
```

### Running the application demo

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

See [ARCHITECTURE.md](ARCHITECTURE.md) for Rust module ownership, data flow,
and the remaining restructuring stages. Run `just check` for the full checks.

Bundled Lua sources live in `scripts/widgets/` and `scripts/components/`,
and seed installed copies once on a fresh installation. Existing directories
are treated as user-owned: deleted Lua files stay deleted across rescans and
restarts. Initialization is recorded outside the directories in
`.widgets-initialized` and `.components-initialized`, so deleting a whole
library directory does not trigger reseeding either. Widget and notification modules return
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
iced.surface({ body = iced.text("Hello"), background = theme.surface,
             border = theme.secondary, border_width = 1, radius = 8, padding = 10 })
iced.styled_button({ label = "Go", action = "go", background = theme.surface })
iced.styled_progress({ value = 0.5, color = theme.primary, background = theme.surface })
iced.styled_separator({ color = theme.secondary, height = 2 })
```

`surface` forwards container sizing and styling, `styled_button` forwards
button sizing, padding, radius, label color and resting background, and
`styled_progress` forwards sizing, fill color and track background.
`card` also accepts `border`, `border_width`, and `separator_color`;
`menu` accepts button `background` and `padding` alongside `color`.

### App-backed shell composables

Shell composables use one optional source-selection table:

```toml
[composable]
selection-rect = "selection_rect.lua"
context-menu = "context_menu.lua"
context-menu-item = "context_menu_item.lua"
```

These are the defaults: omit the entire table unless using custom filenames.
Saves omit default filenames and write only custom sources. Sizing and styling
live in each Lua module's `app.defaults`, rather than TOML property tables.

Relative source paths resolve under `~/.config/riced/components/`; absolute
paths are also supported. The bundled file is seeded without overwriting
existing files. App-backed modules start with `-- riced:composable` so they
are excluded from automatic shared-builder loading, and return an app table:

```lua
-- riced:composable
local app = { defaults = { radius = 4, border_width = 1 } }

function app:view(props)
    return iced.container(iced.space())
        :width(props.width):height(props.height)
        :background(theme.surface)
        :border(theme.primary):border_width(props.border_width):radius(props.radius)
end

return app
```

`props` and `self.props` are the same resolved table: Lua `app.defaults`, then
host-owned state. Rust supplies `width`, `height`, output-local `x`/`y`, `opacity`,
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

### Lua context menu

The background right-click menu and its entries use `context_menu.lua` and
`context_menu_item.lua` by default. Select replacements in `[composable]`.

The seeded frame declares `width = 150`, `height = 92`, `auto_sizing = true`, `padding = 4`,
`spacing = 5`, and `rounding = 3` in `app.defaults`. Entry defaults are
`padding = 5` and `rounding = 3`. Change those defaults directly in Lua;
source edits reload live. Settings exposes the source paths, while appearance
is configured in the selected Lua files.

Rust supplies the frame's `props.items`:

```lua
{
    { label = "Add Top", action = "add-top" },
    { label = "Open Settings", action = "open-settings" },
}
```

It also supplies `props.item_component = { src = ..., props = {} }` and an
`output` geometry table. The frame uses
`riced.component(props.item_component.src, props.item_component.props, item)`
to invoke the item app's `app:view(props)`. Nested apps share the parent's
live theme and instruction/memory limits; their Lua defaults are merged before
host `label`/`action` props. The frame can supply its own overrides through the
second `riced.component()` argument.

With `auto_sizing = true`, the seeded menu's height fits its entries, padding,
and spacing. Disable it to use the numeric `height` property. Width stays
150px by default; set `width = "auto"` in the frame's Lua defaults to fit labels:

```lua
local app = {
    defaults = { width = "auto", height = 92, auto_sizing = true,
        padding = 4, spacing = 5, rounding = 3, border_width = 1 },
}
```

Lua containers accept `:width("auto")` / `:height("auto")` as aliases for
`"shrink"`. In an existing frame script that uses `:height(props.height)`,
setting `height = "auto"` enables content-height sizing directly. Seed files
are not overwritten, so existing frame scripts must opt into the new
`auto_sizing` flag themselves if they do not already use it.

The frame must return a container with positive numeric dimensions or
`"auto"` / `"shrink"` sizing. Iced measures the actual child layout before
positioning it; output-edge clamping and click containment use those measured
bounds, including when a menu is constrained by a small output. Rust owns opening, outside-click dismissal,
and the two action handlers; unknown actions are ignored with a diagnostic.
Broken sources or invalid geometry keep the last working tree, with a native
menu fallback with a fixed width and content-height sizing when no working tree exists.

Old `[composable.<name>]` tables migrate on file load to source strings.
TOML appearance overrides and `[composable.menu]` are retired; transfer any
custom appearance values to the corresponding Lua `app.defaults`. Before
rewriting the config, migration preserves `config.toml.pre-composable-sources`.
The shared `iced.menu` list builder is independent of this retired config table.

Popup/bar/notification frames and interactive settings components remain the
next migration stages.

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
[notification]
enabled = true
output = "mouse"  # or "DP-1" to pin
source = "notifications.lua"
timeout-ms = 5000
```

`source` resolves relative to `~/.config/riced/components/` (absolute paths also work),
using the same resolver as composable sources and `riced.component()`.
It renders the **whole scene**, including the list, through the shared overlay
host. Width, placement, spacing, card height, and transitions belong in Lua:

```lua
local app = {}
function app:view(props)
    return iced.listview(props.notifications)
        :id("notifications"):key("id")
        :delegate(function(n)
            return riced.component("notification_card.lua", n)
                :on_press("dismiss:" .. n.id)
        end)
        :width(360):anchor("right"):margin(16)
end
function app:on_action(key)
    local id = key:match("^dismiss:(%d+)$")
    if id then return { dismiss = tonumber(id) } end
end
return app
```

The bundled `scripts/components/notifications.lua` additionally shows images and action
buttons. `components/notification_card.lua` is the reusable per-card module.
The scene receives `props.notifications` newest-first, filtered to its output,
with `{ id, app, title, body, icon, urgency, actions, has_image, image }`.
`image` is an opaque owned token accepted by `iced.image(n.image)`; image
pixels are never serialized through Lua. `actions` contains `{ key, label }`.
Return `{ invoke = { id = id, key = key } }` to invoke a validated D-Bus action.

### Popup timeout and retained notifications

`timeout-ms` hides the popup; it does **not** delete its notification-center
record. The popup scene receives visible records only. The `notifications`
service array contains all retained records, so the center and badge include
expired popups. Entries expose `popup_visible` and `closed` flags.
History is in-memory for the current daemon session. Retained records own their
image handles until dismissed; completing an exit releases only the visual
ghost and its callback/item references.

Use explicit commands from `app:on_action` or a completion callback:

```lua
function app:on_action(key)
    if key == "dismiss-all" then
        return notification.dismissAll() -- every output, including hidden history
    end
    local id = key:match("^dismiss:(%d+)$")
    if id then return notification.dismiss(tonumber(id)) end
end
```

These functions construct commands: **return** their result from the handler.
Existing `{ dismiss = id }` and `{ invoke = { id = id, key = key } }` returns
also work; `{ dismiss_all = true }` is the dismiss-all command. The bundled
notification-center widget includes a Dismiss all button.

Expiry emits D-Bus `NotificationClosed(id, 1)` once. Clearing already-expired
history does not send another close signal. User dismissal of a live record
emits reason `2`; peer close removes it with reason `3`. Actions are accepted
only for live records and known action keys. A replacement re-shows its popup
with a fresh generation. Removing an output hides its popups but keeps history.
A client-requested timeout of zero means never expire; negative uses the
configured default. Critical notifications do not expire automatically.

### Optional listview completion callbacks

`onEntered`, `onExit`, and `onDisplaced` accept an optional callback that
receives the captured item when that transition completes:

```lua
:onExit({
    x = { to = -200 },
    opacity = { to = 0 },
    duration = 250,
    callback = function(item)
        return notification.dismiss(tonumber(item.id))
    end,
})
```

Without a callback, exit completion only discards the visual ghost; the default
notification scene keeps history. The example explicitly opts into deleting
the retained record **after** its popup exits.

Callbacks run once per completed transition in the host update phase, never
during decoding, layout, or drawing. Re-entering cancels a pending exit callback;
surface teardown and successful app replacement invalidate old callbacks.
An old notification generation cannot dismiss its replacement. Zero-duration
transitions complete on the next update. Callbacks share the bounded Lua
execution rules, and callback errors do not apply a dismissal command.

`[notification]` controls service delivery and expiry. `source = ""` keeps
the service enabled without its default scene. The old `[notifications]`,
`position`, `width`, and `max_visible` presentation fields are removed.

- **Sources**: any D-Bus client (`notify-send` works once riced owns
  the bus name; yields silently to mako/dunst if one runs), plus
  internal events — config/widgets parse errors, template failures,
  theme-regen results (critical ones persist until clicked).
- **Lua layout**: source and component edits reload through the generic scene
  runtime. Replacements are installed only after successful view decoding;
  invalid edits retain the last working runtime and view. Deleted initialized
  scene files are not re-seeded on restart.
- **Behavior**: one warm, passive layer window per output (kept mapped with
  an empty input region when the queue empties), covering the full output — the whole
  stack shows newest-first and scrolls. Click a card to dismiss; popup deadlines are swept on a
  250ms tick that only runs while a visible popup has a deadline. Actions are
  live: the built-in card renders one button per action, and button
  clicks emit `ActionInvoked` back over D-Bus (then dismiss with
  reason `2`); capability `actions` is claimed alongside `body`.
  App images render too: `image-data` pixbufs first, then the
  `image-path` hint, then `app_icon` as a file path — a 36px thumbnail
  left of the card body (themed icon names still render text-only).
  Scripts see `n.has_image` to adapt layout.

### Other overlay scenes and shared layout properties

Additional scenes share the same native `Layer::Overlay` surface on each
output; they do not create a notification-specific native layer:

```toml
[[overlay]]
id = "status"
source = "status.lua"
output = "DP-1"   # omit for every output
[overlay.props]
label = "Status"
```

Each output/scene has independent Lua state. `app:view(props)` receives
configured properties, `props.output = { name, x, y, width, height }`, and
output-local `props.notifications`. Existing services (`system`, `theme`,
`wayland`, and metadata-only global `notifications`) are published before entries.
`app:on_action(key, value)` updates state; optional `app:on_tick()` opts into
250ms periodic updates. Static scenes only re-run for input/config/service
changes or source reloads. Notification commands can be returned from any scene.

Every iced component supports `:anchor(...)`, `:margin(number)`, and
`:on_press(key)`. Anchoring positions its configured/measured size inside its
parent's available area. Use `left`, `center`, `right`, or the six top/bottom
corner/center positions; names are case-insensitive with hyphens or underscores.
`left`/`right` center vertically; `center` centers both axes. Margin is the
anchor inset. Set an explicit width when you want a content column rather than
filling the parent. Chaining still targets the underlying component:
`iced.text("Status"):anchor("right"):size(18):on_press("status")`.

Scenes use native layout measurements for live hit regions and list displacement;
there is no Rust notification card-height constant. Exit nodes retain their last
measured bounds and render inert on a full-output canvas, outside list/scroll
clipping. Shared surfaces stay `Anchor::all()` / `LayerSize::FILL`, with zero native
margins, no reserved screen area, and keyboard interactivity `None`. Only live
interactive bounds accept input; empty space and ghosts are click-through.
Popup/toplevel keyboard/grab policies remain host-owned.

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
mode = "dark"   # "dark" or "light"
variant = "content" # Material You scheme variant for generate-theme
```

First run seeds `~/.config/riced/theme/` with all 11 vendored themes so
they are editable; edits hot-reload on the 500ms `ConfigTick` (same as
`config.toml`). A Theme page in the Settings panel lists themes + a
Dark/Light toggle, persisted via `ConfigPatch::ThemeName/Darkmode`.

## Dynamic theme (colorgen)

`src/theme_gen/` ports `sys/src/colorgen.rs` (`ColorGen.generate`):
`riced generate-theme [--variant V] [--set] [--templates DIR]` rasterizes
the wallpaper set with the shared wallpaper rasterizer
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
freely. `[theme] templates-dir` overrides the dir, `"off"` disables.

Switching themes re-renders templates too (sys `change_theme`): picking a
theme in Settings (or `riced apply-templates [--templates DIR] [--name X]`)
passes the stored theme object through the same find-and-replace off the
update thread, reporting per-template errors without freezing the shell.

Wallpaper edits in Settings are preview-only. Press **Apply** to bake and
publish the new desktop; dynamic-theme generation then runs once from the
applied output composites. Closing Settings does not apply a wallpaper draft.
External changes to `[[background.images]]` rebuild the applied tiles. Theme
generation remains off the update thread and runs only when the selected
theme is `dynamic`.

The daemon regenerates `dynamic.json` only after a successful explicit
Wallpaper **Apply**. Startup/cache restoration, monitor changes, theme
selection, and variant edits do not generate a new palette. Selecting
`dynamic` loads the saved palette; variant changes take effect on the next
Apply. `riced generate-theme` remains an explicit CLI generation command.

Theme-file updates refresh the palette in existing widget VMs rather than
reloading the entire config or discarding staged edits. Color extraction
samples the already-resident applied pixels through a virtual monitor strip
into a bounded 128×128 image, without constructing/encoding/re-decoding a
full-resolution combined wallpaper. This affects palette sampling only;
displayed wallpapers and editor originals stay full resolution.
Lanczos sampling processes one intermediate row at a time, with reusable
filter weights, rather than retaining a full-width 128-row float buffer.
Daemon generation runs on a joined, short-lived worker; completion is reported
after its inputs and thread-local scratch storage have been released.

## Wallpaper editing and quality

### Per-monitor wallpapers

In **Settings → Wallpaper**, choose **Per Monitor**, select a display in the map
or display picker, then choose its image and content fit. **Apply** publishes the
whole draft; **Discard** restores the applied recipe. Switching back to
**Free Form** preserves its original image placements. Each mode keeps its own
recipe, so switching modes does not erase the other layout.

- **Fit**: show the whole image with black margins, preserving aspect ratio.
- **Expand** (default): fill the display, cropping centrally and preserving aspect ratio.
- **Stretch**: fill the display without preserving aspect ratio.
- **Center**: center original pixels at 1:1 native resolution; oversized images are cropped.

Assignments use output names such as `DP-1`, so they survive reconnects and
Wayland ID changes. Disconnected assignments are retained without loading their
files. Unassigned displays and transparent image regions have a black background.
Preview images are clipped to their assigned display and cannot be dragged or
scaled individually in this mode. The output diagram stays fixed and auto-fitted;
clicking a display selects it without dragging or zooming the layout.

```toml
[background]
mode = "per-monitor" # "free-form" is the default for older configurations

[background.monitors.DP-1]
path = "~/Pictures/landscape.png"
fit = "expand"

[background.monitors.DP-2]
path = "~/Pictures/portrait.png"
fit = "center"
```

The daemon only regenerates the dynamic theme after explicit Apply. The explicit
`riced generate-theme` command uses cached output geometry in Per Monitor mode;
apply wallpapers once before running it. Cache restoration accepts older Free
Form manifests, while Per Monitor tiles are rebuilt for the opaque-black format.

### Applying and retaining image quality

The Wallpaper page owns a draft with full-resolution source buffers. Drag,
Ctrl+wheel scale, image properties, add, and remove operations update only
that preview. **Discard** restores the current applied layout; **Apply** runs
a background bake and atomically saves the editable recipe after success.

Each connected output receives one lossless PNG, sampled from the originals
at its native pixel resolution, with scaling, rotation-aware dimensions,
Z-order, and alpha compositing. The small Settings preview is never used as
the source for those pixels. Original files are not rewritten, and original
paths and placements remain in `config.toml` for later editing.

Derived tiles and their manifest live under `$XDG_CACHE_HOME/riced/wallpapers`
(normally `~/.cache/riced/wallpapers`). Startup reuses matching applied tiles;
output layout/resolution changes regenerate them from the editable recipe.
Failed or superseded jobs leave the previous desktop in place. Unreferenced
generated cache generations are removed after publication, while referenced
source files are preserved.

Closing the editor releases its application-owned source buffers. Desktop
surfaces retain only their applied tiles. Lua UI image nodes also reuse
revisioned, full-resolution decoded handles; caching does not introduce
thumbnails or lossy image compression.

Preview reloads reuse unchanged source buffers, decode new or replaced files,
and drop removed sources. File identity includes modification metadata and atomic
replacement identity. Apply retains the accepted full-resolution pixel snapshot.
Startup restores cached output tiles with at most four concurrent PNG decoders;
a failed restoration falls back to baking without publishing a partial tile set.
Lua polling compares owned descriptions directly, preserving unchanged trees
and skipping their animation resynchronization and repaint. Lua views, delegates,
and transition entry points continue executing with their existing budgets and
self-state semantics.

Native window-close confirmation also reclaims unreferenced decoded-image
cache entries and requests allocator trimming after UI/renderer teardown.
Explicit close and Settings-toggle paths schedule the same deferred cleanup,
even if native confirmation is delayed. The preview worker also trims decoder
temporaries after loading. Native close notifications never request another
close for an already-destroyed window.
Large wallpaper preview sources use anonymous, read-only mappings shared by
their pixel data and image handles. Common 8-bit images decode directly into
mapped storage; the last owner releases the mapping even when decoding and
closing happen on different threads. Original files are never mapped or changed.
Closes whose window actually owned decoded images (wallpaper editor sources,
popup trees with image nodes, notification images) additionally repaint every
surface once. Renderer image-cache eviction is insert-gated, so a redraw alone
does not guarantee eviction; iced's texture atlas also retains its pages for
reuse. Other closes stay host-only cleanup.
Applying identical output pixels keeps the existing image handles, avoiding
another texture upload. Applied tiles remain resident, and graphics-driver
allocation pools may retain their capacity even after editor buffers are freed.

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
