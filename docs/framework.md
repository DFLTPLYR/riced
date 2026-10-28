# Framework apps (experimental)

Riced can host opt-in Lua applications as native Wayland layer surfaces.
Existing bars, desktop widgets, notifications, and overlays remain supported.
The application and managed-resource APIs are experimental, not yet a stable
extension SDK.

## Project layout

### Root entry point and commands

When a project contains `root.lua`, it is the frontend entry point and takes
precedence over the legacy `config.lua` `apps` list. It returns the existing app
contract (`view`, optional `mount`/`actions`/`unmount`) and composes its surfaces
with `riced.layer`. New installations put this file directly in the project
directory. `config.lua` retains preferences and extension configuration.

Register shell commands during module loading, in `root.lua` or a module such
as `state.lua` loaded with `require("state")`:

```lua
riced.ipc({
    settings = function(self, args)
        return shell.open_settings()
    end,
    remove = function(self, args)
        -- args is the original CLI argument array, including flags.
        -- Update root-owned state or return a host action here.
    end,
})
```

```sh
riced ipc settings
riced ipc remove --anchor top --output DP-1
```

The installed frontend registers handlers in `state.lua` for `settings`, `add`,
and `remove`. `add` accepts `--anchor`, optional `--output` and `--thickness`;
`remove` accepts `--anchor` and optional `--output`. The command namespace is
user-defined; `settings` opens Settings, it is not a settings CRUD namespace.
`open-settings` and its old `settings` alias now queue the `settings` command.
Handlers receive the owning app as `self` and run under the normal event budget,
with current props/context and managed resources available. They may return the
same host actions as `actions`. Registration rejects duplicate command names.

Commands are delivered once: if multiple output instances register a handler,
the first instance sorted by app id and output name receives it. IPC remains
session-wide. Requests are queued independently so concurrent clients cannot
overwrite one another. CLI success acknowledges queueing, not handler completion;
unknown commands and handler errors are reported by the daemon.
Legacy projects without `root.lua` retain native Settings opening when they
have no registered `settings` handler.

Project `require` caches modules per VM, resolves dotted names relative to the
app root, and rejects circular imports. `state.lua`, `shell/`, `components/`, and
shared `layers/` participate in source reload detection. Failed candidate reloads
retain the working frontend and its handlers. Settings currently retains its
native window host and Lua pages; its opening policy is now a Lua IPC handler.

The installed root declares every surface from Lua: `layers/background.lua`
plus one `layers/bar.lua` per `bars.lua` entry. `layers/bar.lua` embeds widget
files with `riced.widget(name, props, id)`, so each widget keeps its own VM,
services, popup contract, and `actions` handler. The native bar host is off in
the installed `config.lua` (`bar_host = false`), and the background surface is
Lua-owned (`background.disabled = true`) with wallpaper and desktop widgets
still exposed as native leaves. Desktop Settings requests dispatch `settings`
through the same handler as IPC.

The project root is the directory containing the installed `config.lua`
(`$XDG_CONFIG_HOME/riced`, normally `~/.config/riced`).

```text
config.lua
apps/
    bar/
        app.lua
        components/
            status.lua
        assets/
components/
themes/
```

Enable applications explicitly in `config.lua`:

Select a project with `riced -p PATH` (the directory containing `config.lua`).
`riced -c NAME` resolves `./config/riced/NAME` first, then
`$XDG_CONFIG_HOME/riced/NAME`. Without either option, the normal XDG config is
used. Selected projects keep libraries, themes, and preferences in their own
directory; the process working directory is unchanged. IPC remains session-wide.

```lua
return {
    apps = {
        { name = "bar", props = { height = 32 } },
        -- { name = "bar", id = "secondary", output = "DP-2" },
    },
    bar = {}, -- disable legacy bars if replacing them
}
```

Each entry resolves to `apps/<name>/app.lua`. Names and instance IDs accept
letters, digits, `_`, and `-`. IDs default to the app name and must be unique
across entries. Omit `output` (or use `""`) for an independent instance on every
output. Named-output entries resume when the output reconnects. Merely adding
an app directory does not execute it.

## Application contract

Each app returns a table with `view(props)`. Optional methods:

| Method | Behavior |
| --- | --- |
| `mount(context)` | Runs once before the first view of a candidate instance. |
| `actions(event)` | Receives the existing owned UI action/event format. |
| `on_tick()` | Runs on the host's 500 ms polling tick, not on animation frames or service refreshes. |
| `unmount(reason)` | Runs before retiring a mounted instance on reload, removal, native close, or failed candidate validation. |

`context` contains `id`, `name`, `root`, and `output`. It is also available as
`self.context`. Output contains `name`, `x`, `y`, `width`, and `height` in logical
pixels. Props merge `app.defaults` → configured `props` → host-owned `output`.
Resolved props are available as both the view argument and `self.props` during
handlers. They are cleared between entries to release temporary resources.

Apps currently use the trusted widget Lua profile, including native filesystem
and shell access. Hooks have instruction/memory budgets, but synchronous I/O
can still block the UI. Use the managed APIs below instead of `os.execute`
or blocking pipe reads for background work.
Do not perform blocking work in views or assume hook side effects can be rolled
back. Rust releases owned resources even if an unmount hook errors; process
termination is not guaranteed to run Lua cleanup hooks.

## Managed tasks and timers

Framework apps receive reserved methods `spawn`, `pick_file`, `after`, `every`, `subscribe`,
and `cancel`. Do not define app methods with these names.

```lua
function app:mount()
    self.status = "loading"
    self.request = self:spawn({ "hostname" }, {
        timeout_ms = 2000,
        max_output_bytes = 65536,
        -- cwd = "assets", -- relative to this app, or an absolute path
    }, function(result)
        self.status = result.ok and result.stdout or result.error.message
    end)

    self.clock = os.date("%H:%M:%S")
    self.clock_timer = self:every(1000, function()
        self.clock = os.date("%H:%M:%S")
    end)

    self:after(3000, function()
        self.ready = true
    end)
end
```

All three return a numeric cancellation token. `self:cancel(token)` returns
true when an active callback was removed, false when already completed or
cancelled. Explicit cancellation is silent: it does not invoke the callback.
Tokens are local to an app VM; do not exchange them between instances.

`spawn(argv, callback)` also works without options. Arguments are passed
directly to the executable, **not through a shell**. Pipelines and shell syntax
require an explicit shell executable. Child stdin is closed; stdout and stderr
are captured separately and decoded as UTF-8 with replacement for invalid bytes.
The default working directory is the app directory; the shell process's own
working directory is not changed.

Process results contain `ok`, `type = "process"`, `code` (when available),
`stdout`, and `stderr` after normal exit. Nonzero exit adds an `error` object.
Spawn errors, timeout, cancellation, and output overflow return `ok = false`
with `error = { code, message }`. Error codes are `spawn`, `exit`, `io`,
`timeout`, `cancelled`, and `output_limit`. Not every failure includes captured
output. Timer callbacks receive `{ ok = true, type = "timer" }`.

Limits:

- At most 128 active managed callbacks per instance, including subscriptions.
- At most 256 staged start/cancel requests between host drains.
- Process argv: 1–256 NUL-free strings, at most 65536 bytes in total.
- Timeout: 1–86400000 ms; default 30000 ms.
- Output limit: 1–1048576 bytes **per stream**; default 65536 bytes.
- One-shot timers: 1–86400000 ms; recurring timers: 16–86400000 ms.

Timers are fixed-delay: the next delay starts after the previous tick is
handled, not on a catch-up schedule. A busy UI therefore does not accumulate
timer callbacks. All callbacks run under the app's normal Lua budgets on the
host update thread, with fresh `self.props`, `self.context`, and service tables.
The callback receives only its result; capture `self` in the Lua closure.
After a managed callback, only the owning app's Lua view is refreshed.

Create resources in `mount`, `actions`, `on_tick`, or managed callbacks—not in
views, top-level module evaluation, or `unmount`. Candidate requests remain
staged until mount and first-view validation succeed. Reload, removal, output
loss, and runtime drop cancel owned jobs and release callbacks. Old-generation,
cancelled, duplicated, and superseded deliveries are ignored.

Cancellation, timeout, and output overflow kill the process group and reap the
direct child. Dropping an in-flight process future also kills its process group.
Commands that deliberately detach into another session are not supervised daemons;
use a service manager for those. Cleanup on forced process termination cannot
be guaranteed. Returning `nil` from `view` merely hides the layer and does not
stop the app's timers, subscriptions, or tasks.

## Native file picker

```lua
function app:actions(event)
    if event.key == "choose-file" then
        self.picker = self:pick_file({
            title = "Choose wallpaper",
            extensions = { "png", "jpg", "jpeg", "webp" },
        }, function(result)
            if not result.ok then
                self.error = result.error.message
            elseif not result.cancelled then
                self.path = result.path
            end
        end)
    end
end
```

`pick_file(callback)` also works with defaults. It uses the same native `rfd`
dialog backend as the legacy shell; no Zenity executable is required. Backend
support (such as a working desktop portal) is still required by the platform.
Options accept only `title` (NUL-free, up to 1024 bytes) and `extensions` (up to
32 alphanumeric suffixes, 1–32 bytes each, without dots or wildcards). Omit
extensions to allow all files. Filters are picker hints, not file validation.

Results have `type = "file_dialog"`. A selection returns
`{ok=true, cancelled=false, path=...}` with an absolute native path encoded as
UTF-8. Dismissing returns `{ok=true, cancelled=true}` without a path. The backend
does not distinguish dismissal from some backend failures. A non-UTF-8 path
returns `ok=false` with `error.code = "path"`. Only one managed native picker
can be active across apps; another request receives `error.code = "busy"`.

The numeric token works with `self:cancel(token)`. Calls are staged until the
candidate validates, callbacks run on the host update thread, and cancellation
or app teardown silently suppresses late callbacks. Cancellation stops waiting;
closing an already displayed native dialog is backend-dependent. There is no
timeout while the user chooses a file. Picking does not read or save the file.

## Host plugins

A layer can embed host-owned content that has no description equivalent with
`riced.plugin(id[, args])`. The node carries the plugin id and optional args;
the realizing host fills it during layout, and an unknown id is a reported
render error instead of a silently dropped node.

```lua
content = iced.stack({
    riced.plugin("wallpaper"),
    riced.plugin("desktop-widgets"),
})
```

| Plugin | Host | Fills with |
| --- | --- | --- |
| `wallpaper` | framework layer | The baked wallpaper tile published after Apply (cover fit, shader-aware) — the same pixels the native background draws. Empty until the first bake succeeds. |
| `desktop-widgets` | framework layer | The legacy desktop-widget host for that output: every `desktop` placement, with service snapshots, hover, hold-drag, and persistence. |
| `widget` | framework layer | One `widgets/<name>.lua` instance, built by `riced.widget`. |
| `settings-page` / `wallpaper-map` | Settings host | The page body and the wallpaper map canvas. |

Framework layers and the Settings hosts resolve plugins; bar, scene, and
overlay hosts reject them. Plugins that need output geometry require an
output-bound app (every layer app is). Each plugin is Fill-sized, so place it in
a `stack` or `container` rather than a `row`.

Any id that is not a built-in resolves `plugins/<id>.lua` in the project. The
module is a normal app table with `view(self, props)` and optional
`actions(self, event)`; `props` is the `args` table passed to `riced.plugin`,
and its action keys are routed back as `plugin:<id>:<key>`.

```lua
-- plugins/hello.lua
return {
    view = function(self, props)
        return iced.text("Hello " .. (props.name or "plugin"))
    end,
}
```

```lua
-- a layer
content = riced.plugin("hello", { name = "world" })
```

The installed frontend seeds `plugins/hello.lua` and `plugins/counter.lua` as
examples.

`wallpaper` is what lets a Lua background coexist with the native wallpaper
pipeline: placement stays Lua, while baking, shader compilation, the theme
regeneration prompt, and the decoded pixel resource stay native.

Because the plugin is empty until a bake succeeds, a background layer should not
depend on it for being visible. The app also receives the *configured* source
for its output as `props.wallpaper` (per-monitor entry first, then the first
free-form image), so it can draw that path directly underneath:

```lua
if props.wallpaper ~= "" then
    children[#children + 1] = iced.image(props.wallpaper):width(size.fill):height(size.fill)
end
children[#children + 1] = riced.plugin("wallpaper")
```

`wallpaper` is what lets a Lua background coexist with the native wallpaper
pipeline: placement stays Lua, while baking, shader compilation, the theme
regeneration prompt, and the decoded pixel resource stay native.

Because the slot is empty until a bake succeeds, a background layer should not
depend on it for being visible. The app also receives the *configured* source
for its output as `props.wallpaper` (per-monitor entry first, then the first
free-form image), so it can draw that path directly underneath:

```lua
if props.wallpaper ~= "" then
    children[#children + 1] = iced.image(props.wallpaper):width(size.fill):height(size.fill)
end
children[#children + 1] = iced.native("wallpaper")
```

`iced.image` uses contain fitting, so the direct draw is a fallback, not the
final look; the baked tile covers it once it exists. It also means a bake
failure shows a wallpaper rather than a blank screen.

## Host shell commands

An action handler can ask the daemon to perform a shell-level action by
returning a value. Nothing runs during rendering: the value travels back through
the same action boundary as tray and notification commands.

```lua
function app:actions(event)
    if event.key == "open-settings" then
        return shell.open_settings()
    end
end
```

| Call | Effect |
| --- | --- |
| `shell.open_settings()` | Toggle the Settings window (same as the CLI `open-settings` command). |

Available to every Lua host — framework apps, bar widgets, scenes, and popups.
Returning any other value, or `nil`, stays inert. A malformed command
(`{shell={action="nope"}}`, extra fields, a non-table) is reported and ignored
rather than executed. Apps cannot add new shell actions; the set is fixed by the
host, and the daemon owns execution and validation.

**Layer creation is deliberately not a shell action.** An app declares its
layers in `view` and owns any layer *list* as its own data, so the runtime never
invents a surface on request. See [App-owned layers](#app-owned-layers).

### Bars are app data

A bar placement is app-owned Lua data, not a host command:

- Bars are files under `data/bars/`. `root.lua` lists the directory with
  `riced.list`, reads each with `riced.read`, and composes one `layers/bar.lua`
  per file. Settings → Panel lists the same directory and writes the files with
  `riced.write`. There is no `shell.add_bar`/`shell.remove_bar` and no hardcoded
  bar list.
- `layers/bar.lua` builds the surface and embeds widgets with
  `riced.widget(name, props, id)`. Each widget keeps its own VM, services,
  popups, and `actions`; the layer only decides where it sits.
- Give each bar a **stable identity** — one bar per (output, edge) — so removing
  one never renumbers the others. Bundled bars without a target keep
  `data/bars/<edge>.lua` and appear on all outputs. Targeted bars use
  `<edge>-output-<hex-encoded-name>.lua`, retain the original connector in
  `output`, and appear only there. Add and remove IPC accept `--output NAME`;
  removal never falls back to the all-output file. Disconnected targets keep
  their files. The data uses `edge`/`thickness`/`slots`, where each slot is
  `{ align = "start"|"center"|"end", widgets = { "name", ... } }` and
  `layers/bar.lua` iterates the ordered list. A layer should still write
  `bar.edge or "top"` so a malformed document cannot take the frontend down.
- Widget popups are declared by the same layer with `riced.widget_popup({x, y})`;
  the host renders the active widget's popup content there.
- Settings → Panel is a persistent Lua page: it keeps its VM, lists and edits
  these same files, shows their output names, and preserves the target on edits.
  Its Add buttons still create all-output bars; no output-picker UI is required.

An action handler can therefore offer "add a bar" and "remove this bar" by
editing the app's own file:

```lua
function app:add_bar(edge)
    local bars = self.bars:get("bars") or {}
    table.insert(bars, { edge = edge, thickness = 34 })
    self.bars:set("bars", bars)
    self.bars:save()
end
```

### App-owned layers

Layers that are *not* configuration — a background, a HUD, a widget board — are
still the app's own. The layer set is not fixed by `config.lua`; an app can
declare as many as it likes, including ones derived from its own state:

An app is either `apps/<name>/app.lua` (a folder app, with app-local
`components/`) or a single file `apps/<name>.lua`. Either way `layers/` at the
project root is shared, so a layer module is
`riced.layer("layers/bar.lua", props)`. A single-file app still gets
`apps/<name>/` for owned data such as `state/`, never for its entry point.

```lua
function app:mount(context)
    self.output = context.output.name
    self.bars = riced.file({ type = "lua", path = "bars.lua",
                             write_mode = "generated", defaults = { bars = {} } })
end

function app:add_bar(edge)
    local bars = self.bars:get("bars") or {}
    table.insert(bars, { id = "bar-" .. (#bars + 1), edge = edge, thickness = 32 })
    self.bars:set("bars", bars)
    self.bars:save()
end

function app:view(props)
    local layers = { riced.layer("layers/background.lua", { menu = self.menu }) }
    for _, bar in ipairs(self.bars:get("bars") or {}) do
        layers[#layers + 1] = riced.layer("layers/bar.lua", {
            id = bar.id, output = self.output,
            edge = bar.edge, thickness = bar.thickness,
        })
    end
    return layers
end
```

"Add another bar" is now ordinary app state: append, save, re-declare. The
daemon still owns validation, identity, persistence, and window creation — Lua
owns the policy of *when* a layer appears and what it is told.

Store a **stable identity** in the document (here `bar.id`), never an array
index: removing `bar-1` must not renumber `bar-2`, or the runtime closes and
recreates unrelated surfaces. The same document can be edited by Settings and by
the layer repeater, which is what keeps them consistent.

A document opened with `riced.file` is data, not app code, so editing it never
reloads the app. Keep it outside `apps/<name>/components/`, which is the app's
source set.

### Which layer sent an action

Every layer of one app shares a Lua state, so an action handler must be able to
tell them apart. The action event carries the id of the layer it came from:

```lua
function app:actions(event)
    if event.type == "press" and event.button == "right" then
        if event.layer == "background" then
            self.menu = { x = event.x, y = event.y }
        else
            self.bar_menu = { id = event.layer, x = event.x, y = event.y }
        end
    elseif event.key == "remove:" .. tostring(event.layer) then
        -- ...
    end
end
```

`event.layer` is the surface id of the layer that owns the surface the pointer
hit. A right-click on a bar therefore reaches the app as
`{type="press", button="right", layer="bar-2", x=…, y=…}`, which is enough to
drop a per-bar menu without a native menu host: declare it as that layer's
`popup`.

## Unified service snapshots and subscriptions

```lua
function app:mount()
    local service = riced.service("system")
    local initial = service:snapshot()
    self.cpu = initial.data.cpu_usage

    self.cpu_subscription = service:subscribe(function(snapshot)
        self.cpu = snapshot.data.cpu_usage
    end)

    -- Equivalent convenience method:
    self:subscribe("wayland.workspaces", function(snapshot)
        self.workspaces = snapshot.data
    end)
end
```

Snapshots use `{ name, available, data }`. `available` means the published topic
exists, not that a compositor implements every underlying protocol. Topics are
`system`, `theme`, `notifications`, `wayland`, `systemtray`, `wayland.outputs`,
`wayland.workspaces`, and `wayland.toplevels`. Unknown names fail immediately.

`snapshot()` reads cached published data without filesystem, D-Bus, or Wayland
requests. It returns detached plain data; mutating it does not edit the service.
Functions and opaque image tokens are omitted. Image-only changes therefore do
not constitute metadata snapshot changes; existing service globals remain the
API for opaque image values and existing native command APIs remain unchanged.
Generic service commands and service registration belong to the extension SDK
milestone, not this read/subscription interface.

Subscriptions deliver one initial snapshot asynchronously, then changed data
only. Multiple pending updates coalesce to the latest snapshot. Source changes
arrive through the existing native service events or the 500 ms app poll;
callbacks never run recursively inside publication. Cancel subscription tokens
with `self:cancel(token)`, or let app teardown release them automatically.

## Layer roots

```lua
local app = { defaults = { height = 32 } }
function app:view(props)
    return {
        iced.layer({
            id = "wallpaper",
            layer = "background",
            anchors = { top = true, right = true, bottom = true, left = true },
            exclusive_zone = -1,
            pointer_events = true,
            content = iced.image(props.wallpaper):width(size.fill):height(size.fill),
        }),
        iced.layer({
            id = "bar",
            layer = "top",
            anchors = { top = true, left = true, right = true },
            height = props.height,
            exclusive_zone = props.height,
            keyboard = "none",
            input = "all",
            content = iced.text("Hello " .. props.output.name),
        }),
    }
end
return app
```

An app declares **one layer, an array of layers, or `nil`**. Each entry owns its
own native surface, list state and measurement, so one app can cover the
background, a bar and an overlay at once. `nil` hides every layer without
discarding application state. Layer roots cannot appear in widget trees.

A layer can also live in its own file: `riced.layer("layers/bar.lua", props)`
resolves `<app>/components/layers/bar.lua` (or the shared `components/`), calls
its `view(props)`, and returns its `iced.layer{...}` value for the list. The
module owns its surface, not just its content, so it decides its own level,
anchors and input semantics. A file that returns a widget tree is rejected by
name. `riced.layer` is deliberately distinct from `riced.component`, which
returns a subtree.

Ids must be unique across the list: each one keys a native surface, and a
duplicate is a reported render error. A layer that disappears from the list has
its surface closed; a layer whose `id` changes gets a fresh surface without a new
VM. Closing any declared layer closes the whole app, since the app declares the
set.

| Field | Contract |
| --- | --- |
| `id` | Nonempty surface identity; defaults to `"main"`. Changing it recreates the native surface, not the Lua instance. |
| `layer` | `"background"`, `"bottom"`, `"top"` (default), or `"overlay"`. |
| `anchors` | Named booleans: `top`, `right`, `bottom`, `left`; all default false. |
| `width`, `height` | Integer logical pixels in `1..32768`. An omitted dimension stretches and requires both opposite anchors. |
| `exclusive_zone` | Integer `-1..32768`, default `0`; positive values reserve space according to Wayland layer-shell rules. |
| `margin` | Named integers `top`, `right`, `bottom`, `left`, default zero. |
| `keyboard` | `"none"` (default), `"on_demand"`, or `"exclusive"`. |
| `input` | `"all"` (default), `"none"`, or `"regions"`. `"regions"` unions the measured bounds of clickable widgets and controls, so the rest of an oversized layer stays click-through. |
| `pointer_events` | Boolean, default false. Deliver unconsumed layer-local pointer gestures to `actions`; child controls handle input first. |
| `shared_pointer_events` | Boolean, default false. Mirror unconsumed pointer presses and grabbed gestures to the same app/layer on every output. Requires `pointer_events=true`, all four anchors, `exclusive_zone=-1`, no fixed dimensions, and zero margins. |
| `content` | Required ordinary `iced.*` widget tree. |

With `input = "regions"`, widgets that handle input (`on_press` mouse areas,
buttons, sliders, text inputs, and other controls) contribute surface-local hit
rectangles. Wrap any node in `iced.region(child)` to add an explicit hit area
even when it has no action. A full-output or oversized layer can therefore
paint a large drawer while remaining click-through everywhere else; when there
are no regions the input mask is empty.

Layer-app pointer moves are coalesced to the latest position on each redraw.
Pending motion is delivered before release or pointer exit, so drag endpoints
are not lost. A background-owned press retains its gesture outside the surface
until release; focus loss sends `type="cancel"`. Child controls cannot steal an
already-started background drag, and their own presses never start one.
Input normally refreshes only its owning app instance without rescanning
source files; hot reload remains handled by the regular project refresh.
Content-only changes redraw the affected surface rather than every window.

Shared gestures translate `event.x/y` into each recipient output's local logical
coordinates (possibly negative/outside its bounds); add `self.props.output.x/y`
to obtain the same global point in each VM. `event.source_output` names the
originating output. Gate one-output side effects such as opening a context menu
on `event.source_output == self.props.output.name`. Keyed control actions and
idle hover/motion are not broadcast, nor are gestures shared with other apps or
layers that did not opt in. No Lua objects or persistent state cross VM boundaries.

The installed root uses this transport for desktop selection: Lua owns geometry,
an inert canvas clips the global fill/border on each output, and `:animate` fades
it with the native clock on release. A new drag gets a fresh animation identity;
no repeating Lua timers or disk-backed gesture state are needed.

Unknown fields and invalid geometry fail before native creation. Layout, focus,
layer, margins, input, and reserved space reconcile on the same native window.
Native UI realization/animation uses cached owned trees, never per-frame Lua.

## App-owned popups

A framework layer can declare one named popup per app/output instance. Its
content is an ordinary widget tree and shares the parent's Lua state:

```lua
function app:actions(event)
    if event.type == "popup_closed" then
        self.menu_open = false
    elseif event.key == "menu" then
        self.menu_open = not self.menu_open
    end
end

function app:view(props)
    local menu
    if self.menu_open then
        menu = {
            id = "menu", x = 16, y = 48, width = 260, height = 160,
            content = iced.container(iced.text("Shared app state"))
                :width(size.fill):height(size.fill):padding(16)
                :background(theme.surface):radius(12),
        }
    end
    return iced.layer({
        anchors = { top = true, left = true }, width = 180, height = 32,
        content = iced.button("Menu", "menu"), popup = menu,
    })
end
```

- `id` defaults to `popup`: 1..64 ASCII letters/digits/`_`/`-`. `width` and
  `height` are required integers in 1..32768. `x`/`y` default to zero and are
  finite logical-pixel coordinates relative to the **output**, not the parent
  layer or a widget. The card's size and position are clamped to that output.
- `content` is required. It cannot contain `iced.layer` or `iced.popup` roots.
  List IDs must be unique across the parent's and popup's widget trees.
- The host uses a full-output, input-capturing overlay layer with exclusive
  keyboard focus and no exclusive zone. Outside left/right/middle clicks and
  Escape dismiss it; blank space inside the card consumes clicks. This is a
  modal host-owned layer, not a native `xdg_popup` or compositor popup grab.
- Dismissal invokes `actions` with
  `{type="popup_closed", id="menu", reason="outside"|"escape"|"closed"}`.
  Widget actions inside the card enter the same `actions` handler as the parent.
  Action keys should distinguish the controls in both surfaces.
- A dismissed ID stays suppressed even if the handler errors or leaves the
  declaration unchanged. Return `popup=nil` on an accepted view before
  reopening that ID. Removing the declaration closes it without a dismissal
  event; changing its ID creates a fresh popup.
- Parent hiding/identity changes, app removal, successful reload, output
  removal, and parent native close release the child. Closing only the popup
  does not unmount the parent. Stale child events cannot enter a replacement.
  Invalid replacement views preserve the previous parent and popup.
- Content updates reuse the child surface; list animation state, hover, and
  measurements remain scoped to its owner. Popup open/close lifecycle
  transitions, automatic widget anchoring/flipping, nested popups, and ordinary
  application windows are not part of this step. Existing bar-widget popup
  behavior is unchanged.

See `examples/apps/popup/app.lua` for a runnable counter/menu example.

## Components, assets, and reload

`riced.component("status.lua", props)` prefers
`apps/<name>/components/status.lua`, then falls back to shared components.
App-local components return the existing component app-table contract.
Shared pure `iced.define` registrations continue to load in sorted order.
Use `riced.path("assets/icon.png")` for a path relative to the app directory;
the process working directory is never changed.

The host polls `app.lua`, app-local `components/**/*.lua`, and shared definitions.
Other files, including generated Lua preferences, are not application source.
A changed source
loads in a fresh VM, mounts, and validates its first layer view before replacing
the active runtime. Syntax, mount, and decode failures preserve the previous
surface. Corrected sources are retried. Successful reload resets app state;
persistent state migration is not implemented yet.

See `examples/` for a Lua bar with a local component, clock, and
independent click counter on each output. Copy its `apps/bar/` directory into
your project and merge the example `apps` entry into your existing config.

Run `riced -c example` from the repository root for the background migration
sample in `config/riced/example/`. It demonstrates wallpaper preferences, file
watching, a context menu, and local selection with a fade. Its README lists
remaining native-only features. `background = { disabled = true }` disables
legacy background creation on startup; restart after changing it. This also
disables the legacy desktop-widget host, not just its wallpaper rendering.

## Subsequent milestones

The [structured persistence API](persistence.md) provides `riced.file()` with
cached edits, managed reload/save operations, conflict detection, and coalesced
file watching. The [experimental Rust extension SDK](extensions.md) provides
versioned registration, cached service snapshots, and managed service commands
for compiled-in Cargo crates under `extensions/<name>/`. Extensions are enabled
in `config.lua`, and `riced build` generates the registration entry point plus
a locked Cargo package under `.riced/`, so adding an extension no longer edits
framework source. The palette extension demonstrates this owned-data interface
without applying shell theme changes. Compiled-in extensions require
rebuilding/restarting. Supervised process services now provide lazy executable
startup, capability negotiation, bounded commands, cancellation/deadlines,
crash detection, and restart-on-next-request through the same Lua service API.
See [process configuration and limits](extensions.md#supervised-executable-services).
Rust dynamic-library loading is
intentionally not part of the initial design.

The application-model work now includes one modal, named popup owned by each
framework app/output instance. **Next: ordinary application-window roots** for
apps such as Settings. Automatic widget anchoring/flipping, popup lifecycle
transitions, nested popups, and explicit state restoration remain follow-ups;
the existing native bar-popup host keeps its current capabilities.

### Later: Lua editor support (LuaLS)

Provide completion, signatures, hover documentation, and lightweight type
checking through `lua-language-server` and LuaCATS definitions, without a custom
language server or changes to runtime behavior. Schedule full coverage after
the extension SDK's public Lua surface stabilizes; core `iced.*` definitions
can land earlier.

Planned work:

1. **API definitions:** add `types/riced.lua` as a `---@meta` library covering
   widget constructors and chainable setters; layer roots; animations and
   components; app lifecycle, managed methods, and events; persistence handles;
   service snapshots; and notification/tray commands. Derive names and fields
   from Rust schemas and publication code rather than documentation alone.
2. **Editor setup:** add a Lua 5.4 `.luarc.json` with `types` as a workspace
   library, plus configuration for opening `config/riced/example/` directly.
   Document Neovim and VS Code setup in the Lua API guide.
3. **Example annotations:** describe the background app's custom state with a
   class extending `RicedApp`, and annotate component props and delegate items
   where useful. Explain that runtime-defined components need explicit type
   declarations or annotations; `iced.use` can provide a statically described
   entry point but cannot infer arbitrary component props or return types.
4. **Drift checks:** test that every Rust component constructor and setter has
   a corresponding declaration. Introduce generation from schema metadata if
   maintaining the definitions manually becomes cumbersome.
5. **Optional distribution:** later consider `riced lua-stubs [PATH]` to export
   definitions and editor configuration for standalone projects. Define an
   explicit overwrite policy before writing user-owned configuration.

Acceptance criteria:

- Opening the repository or background example provides constructor, setter,
  and service completion; chained setters preserve the widget's specific type.
- The annotated example passes LuaLS checks without undefined-global or
  setter-type errors, and representative invalid calls produce diagnostics.
- Adding a constructor or setter without updating definitions fails a test.
- Documentation distinguishes inferred API types from app-specific annotations.

Validate LuaLS's generic/self-preserving return behavior before choosing the
shared-setter model. If unreliable, declare setters separately per widget class
to preserve chaining. Arbitrary runtime component discovery, automatic typing of
all app state, and replacing Rust runtime validation are explicitly out of scope.
