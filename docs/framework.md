# Lua shell and apps (experimental)

The bundled shell uses one Lua daemon to declare surfaces across all outputs.
Standalone apps can instead run with independent state per output. This guide
covers both models; widget constructors are in the [Lua API](lua-api.md).
The application and managed-resource APIs are experimental.

## Project layout

### Shell entry point and commands

When a project contains `shell.lua`, it is the frontend entry point and takes
precedence over the `data/config.lua` `apps` list. It returns a daemon via
`riced.daemon(...):run()`, or a plain app table with `view` and optional
`mount`/`actions`/`unmount`. It composes surfaces with `riced.layer`/`iced.window`.
New installations put this file directly in the
project directory. `data/config.lua` retains preferences and extension
configuration; older projects may still keep a top-level `config.lua`, which is
imported when `data/config.lua` is absent, keeping the original. `root.lua`
remains the fallback when `shell.lua` is absent.

Register shell commands during module loading, in `shell.lua` or a module such
as `data/state.lua` loaded with `require("data.state")`:

```lua
riced.ipc({
    settings = function(self, args)
        self.settings_open = not self.settings_open
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

The installed frontend registers handlers in `data/state.lua` for `settings`, `add`,
and `remove`. `add` accepts `--anchor`, optional `--output` and `--thickness`;
`remove` accepts `--anchor` and optional `--output`. The command namespace is
user-defined; `settings` opens Settings, it is not a settings CRUD namespace.
Application commands use only `riced ipc <command> args`; the old
`riced open-settings` and `riced settings` spellings have been removed.
Handlers receive the owning app as `self` and run under the normal event budget,
with current props/context and managed resources available. They may return the
same host actions as `actions`. Registration rejects duplicate command names.

Commands are delivered once: if multiple output instances register a handler,
the first instance sorted by app id and output name receives it. IPC remains
session-wide. Requests are queued independently so concurrent clients cannot
overwrite one another. CLI success acknowledges queueing, not handler completion;
unknown commands and handler errors are reported by the daemon.
Projects must register their own `settings` handler; there is no native Settings
command fallback.

Project `require` caches modules per VM, resolves dotted names relative to the
app root, and rejects circular imports. `data/state.lua`, `shell/`, `components/`,
and `composables/` participate in source reload detection. Failed candidate reloads
retain the working frontend and its handlers. Settings currently retains its
native window host and Lua pages; its opening policy is now a Lua IPC handler.

### The daemon builder

The bundled `shell.lua` returns `riced.daemon(...):run()`. One Lua VM
owns all outputs, and `view` declares every surface for every output.

```lua
local app = {}
function app.new() return app end
function app.namespace() return "riced" end
function app.update(self, event)
    if event.key == "refresh" then self.refreshed = true end
end
function app.view(self, props)
    local layers = {}
    for _, output in ipairs(props.outputs) do
        layers[#layers + 1] = iced.layer({
            id = output.name .. ".status", output = output.name,
            anchors = { top = true, left = true }, width = 200, height = 32,
            content = iced.text(self.refreshed and "Refreshed" or "Ready"),
        })
    end
    return layers
end

return riced.daemon(app.new, app.namespace, app.update, app.view)
    :title(function(self, surface) return surface.title end)
    :theme(function(self, context) return context.theme end)
    :style(function(self, context) return { background_color = "#00000000", text_color = context.text_color } end)
    :redraw_scope(function(self, event) return event.scope end)
    :settings({
        id = "riced",             -- compositor app id; empty = unset
        antialiasing = false,
        default_text_size = 16,
        keep_compositor_alive = true,
        tracing = "riced=debug,warn", -- RUST_LOG-style filter
        layer_settings = {
            start_mode = "background",          -- background | all_screens
            keyboard_interactivity = "none",    -- none | on_demand | exclusive
        },
    })
    :run()
```

`new` returns the mutable state table. `namespace` returns a nonempty, NUL-free
string. `update` runs on the host thread for UI actions, `tick`, `output_added`,
`output_changed`, `output_removed`, `services_changed`, and `config_changed`;
its return value is handled like `actions`. `title`, `subscription`, `theme`,
`style`, and `redraw_scope` may be omitted; the native defaults apply.
`redraw_scope` receives `{kind, scope, surface}` and returns `"all"`, `"none"`,
or `"window"`.

`:subscription(callback)` replaces the host's optional event sources. The
callback receives activity flags and returns a dense array of source names:
`input`, `wayland`, `config`, `ipc`, `tray`, `desktop`, `apps`, `settings`,
`animation`, `notification_deadlines`, `overlays`, `notifications`, `selection`,
and `widgets`. Omitting `apps` disables the regular 500 ms app poll used for
ticks, source reload checks, and managed-resource pumping. Other refresh events
may still run, but do not rely on them to replace that poll.
Native surface lifecycle events always remain enabled. The bundled shell's
`app:subscription` shows how to retain active sources.

`view` declares surfaces; each `iced.layer` needs an `output` name from
`props.outputs` because the daemon is not bound to one output. Closing a surface
drops that layer but keeps the daemon and its state. Changing between a daemon
and a plain app requires a restart.

Settings are validated before connecting. Unknown keys, wrong types, and a
non-finite or non-positive `default_text_size` reject the load. A bad `subscription` result
(unknown, duplicate, or non-array source) is reported and the native defaults are
used for that pass. Edit startup options and restart; there is no live reload. A
missing `tracing` filter keeps `RUST_LOG`, then `info`. Keep top-level code
side-effect free; put effects in `mount`/`update`.

Preferences never live in `shell.lua`. Settings rewrites
`data/config.lua` as a plain `return {...}` table, so hand-written views are
never overwritten.

The installed shell declares every surface from Lua: `components/layers/background.lua`
and `components/layers/notifications.lua`, plus one `components/layers/bar.lua`
per `data/bars/*.lua` entry. `layers/bar.lua` embeds
widget files with `riced.widget(name, props, id)`, so each widget keeps its own
VM, services, popup contract, and `actions` handler. The native bar host is off
in the installed `data/config.lua` (`bar_host = false`), and the background
surface is Lua-owned (`background.disabled = true`) with wallpaper and desktop
widgets still exposed as native leaves. Desktop Settings requests dispatch
`settings` through the same handler as IPC.

The project root is the directory containing `shell.lua`
(`$XDG_CONFIG_HOME/riced`, normally `~/.config/riced`).

```text
shell.lua
data/
    config.lua
    state.lua
    bars/
    plugins/
    theme/
    templates/
    state/                 initialization markers and locks
components/
    *.lua                  shared builders
    layers/
    settings/
        window.lua
    shaders/
composables/
widgets/
apps/
    bar/
        app.lua
        components/
            status.lua
        assets/
```

For projects without a shell entry, enable apps in `data/config.lua`:

Select a project with `riced -p PATH` (the directory containing `shell.lua`).
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

Each entry resolves to `apps/<name>/app.lua`, or `apps/<name>.lua` when no folder
entry exists. Names and instance IDs accept
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
For a daemon, `props.outputs` lists all connected outputs, each with geometry
and its configured `wallpaper` path; it does not have a single owning output.
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

Framework layers and the Settings hosts resolve plugins; legacy bar, scene, and
overlay hosts reject them. Plugins that need output geometry use the layer's
target output. Each plugin is Fill-sized, so place it in
a `stack` or `container` rather than a `row`.

Any id that is not a built-in resolves `data/plugins/<id>.lua` in the project
(with `plugins/<id>.lua` as a legacy fallback). The
module is a normal app table with `view(self, props)` and optional
`actions(self, event)`; `props` is the `args` table passed to `riced.plugin`,
and its action keys are routed back as `plugin:<id>:<key>`.

```lua
-- data/plugins/hello.lua
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

The installed frontend seeds `data/plugins/hello.lua` and `data/plugins/counter.lua` as
examples.

`wallpaper` is what lets a Lua background coexist with the native wallpaper
pipeline: placement stays Lua, while baking, shader compilation, the theme
regeneration prompt, and the decoded pixel resource stay native.

Because the plugin is empty until a bake succeeds, a background layer should not
depend on it for being visible. The app also receives the *configured* source
for its output as `props.wallpaper` (the matching Per Monitor assignment or the
first image in Free Form mode; empty when missing). A daemon receives these
paths on `props.outputs` and passes the relevant one to its background module:

```lua
if props.wallpaper ~= "" then
    children[#children + 1] = iced.image(props.wallpaper):width(size.fill):height(size.fill)
end
children[#children + 1] = riced.plugin("wallpaper")
```

`iced.image` uses contain fitting, so the direct draw is a fallback, not the
final look; the baked tile covers it once it exists. It also means a bake
failure shows a wallpaper rather than a blank screen.

## Native windows

`iced.window({...})` declares an XDG toplevel alongside `iced.layer({...})` in
an app's `view`. Lua owns visibility; removing a declaration closes its surface.
Stable ids reuse native windows across renders. The bundled `components/settings/window.lua`
declares the Settings window and embeds `riced.plugin("settings")`, whose native
editors preserve existing validation and Apply/Discard behavior.

```lua
function app:actions(event)
    if event.key == "settings" then
        self.settings_open = not self.settings_open
    elseif event.type == "window_closed" and event.id == "settings" then
        self.settings_open = false
    end
end

function app:view()
    if not self.settings_open then return {} end
    return iced.window({
        id = "settings", title = "Riced Settings", width = 1100, height = 760,
        content = riced.plugin("settings"),
    })
end
```

Native closes deliver `{type="window_closed", id=...}` to the owning app's
`actions` (or the daemon's `update`). Closing a window does not unmount the app
or close its bars. Failed close handlers cannot reopen it accidentally: omit
the declaration once before
declaring that id again. `shell.open_settings()` and the `shell` namespace have
been removed; internal UI actions change Lua state without IPC polling.

Window fields are `id`, `title`, `width`, `height`, `client_side_decorations`,
and `content`. Defaults are `main`, `Riced`, 960×720, and server decorations.
Sizes must be integers in 1..32768. Size/title changes reuse the window;
decoration or surface-kind changes recreate it. Layer-only properties and
`popup` are rejected on windows. Windows cannot nest inside widget trees.

### Built-in palette commands

`iced.palette()` reads an owned snapshot of the applied shell palette in every
Lua host. Return `iced.palette({primary="#89b4fa", on_primary="#181825"})`
from an action or managed callback to apply a validated partial update through
the host's existing config/save/theme-refresh pipeline. Constructor calls do
not mutate the host, and palette values are invalid widget/view roots.
The setter returns an opaque native value, dispatched directly without a host
command or mutable table.

Overrides persist in `theme.palette`. Selecting a named theme clears them;
changing dark/light mode keeps explicit colors. `theme.*` is a compatible
snapshot of the same values. Setting colors does not apply external-app templates.
See [the palette contract](lua-api.md#icedpalette-built-in-not-a-widget).

Extensions do not receive shell-state access. A generated palette can be applied
by returning `iced.palette(result.data.colors)` from its accepted Lua callback.
Existing cancellation/reload ownership checks reject late callbacks before any
palette command is delivered. The service's generated snapshot is not applied
palette state; use `iced.palette()` to read the latter.

### Bars are app data

A bar placement is app-owned Lua data, not a host command:

- Bars are files under `data/bars/`. `shell.lua` lists the directory with
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
  `bar.edge or "top"` for a missing edge; other invalid values can still fail
  view validation and leave the last working surfaces in place.
- Widget popups are declared with `riced.widget_popup({owner=prefix})` in the
  layer's `popup` field. Use a unique widget-ID prefix per bar/output so only
  the owning layer declares the popup.
- Settings → Panel is a persistent Lua page: it keeps its VM, lists and edits
  these same files, shows their output names, and preserves the target on edits.
  Its Add buttons still create all-output bars; no output-picker UI is required.

For the bundled read/write handlers, see `scripts/frontend/shell.lua` and
`scripts/frontend/state.lua`. For asynchronous app-owned documents, use
[`riced.file`](persistence.md); it requires a reload before the first save.

### App-owned layers

Layers that are *not* configuration — a background, a HUD, a widget board — are
still the app's own. The layer set is not fixed by `data/config.lua`; an app can
declare as many as it likes, including ones derived from its own state:

An app is either `apps/<name>/app.lua` (a folder app, with app-local
`components/`) or a single file `apps/<name>.lua`. The project's
`components/layers/` directory is shared, so a layer module is
`riced.layer("layers/bar.lua", props)`. A single-file app still gets
`apps/<name>/` for owned data such as `state/`, never for its entry point.

Layers can come from in-memory state or a managed document. Store a **stable
identity** for each layer, never an array index: removing one must not renumber
or reuse another layer's ID. The bundled frontend uses output-name and file-name
identities so its layers remain unique across outputs.

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
Extension services also support managed commands; see the
[extension guide](extensions.md#lua-service-commands). Built-in topics remain
read-only through this interface.

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
resolves an app-local `layers/bar.lua`, `composables/layers/bar.lua`, or
`components/layers/bar.lua`, then the same paths in the project, calls
its `view(props)`, and returns its `iced.layer{...}` value for the list. The
module owns its surface, not just its content, so it decides its own level,
anchors and input semantics. A file that returns a widget tree is rejected by
name. `riced.layer` is deliberately distinct from `riced.component`, which
returns a subtree.

Ids must be unique across the list: each one keys a native surface, and a
duplicate is a reported render error. A layer that disappears from the list has
its surface closed; a layer whose `id` changes gets a fresh surface without a new
VM. For a plain per-output app, closing a declared layer retires that instance.
For a daemon, it dismisses only that layer and keeps shared state alive; omit
the declaration before declaring that ID again to reopen it.

| Field | Contract |
| --- | --- |
| `id` | Nonempty surface identity; defaults to `"main"`. Changing it recreates the native surface, not the Lua instance. |
| `output` | Optional connector name for a plain app; required for daemon layers. Must be nonempty and NUL-free. A disconnected target has no surface until it returns. |
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

For plain per-output apps, shared gestures translate `event.x/y` into each recipient output's local logical
coordinates (possibly negative/outside its bounds); add `self.props.output.x/y`
to obtain the same global point in each VM. `event.source_output` names the
originating output. Gate one-output side effects such as opening a context menu
on `event.source_output == self.props.output.name`. Keyed control actions and
idle hover/motion are not broadcast, nor are gestures shared with other apps or
layers that did not opt in. No Lua objects or persistent state cross VM boundaries.

The bundled daemon does not broadcast between VMs: it stores one global desktop
selection. `event.output` contains the source output's geometry; add its `x/y`
to layer-local pointer coordinates to get global coordinates. Each output's
canvas clips the same rectangle, and `:animate` fades it on release without Lua
frame timers.

Unknown fields and invalid geometry fail before native creation. Layout, focus,
layer, margins, input, and reserved space reconcile on the same native window.
Native UI realization/animation uses cached owned trees, never per-frame Lua.

## App-owned popups

A framework layer can declare one named popup per app instance (one across all
outputs for a daemon). Its
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
  finite logical-pixel coordinates relative to the **declaring parent layer**,
  not the output or a widget. Negative coordinates are allowed (for example,
  `y = -160` opens a 160px-high menu above a bottom bar). The compositor may
  slide or resize the popup to fit its constraints.
- `content` is required. It cannot contain `iced.layer` or `iced.popup` roots.
  List IDs must be unique across the parent's and popup's widget trees.
- The host creates a native `xdg_popup` parented to the declaring layer, sized
  to the card rather than the output. There is no fullscreen backdrop or
  layer-shell keyboard-focus override. Pointer-triggered opening uses the
  backend's available input serial for a compositor grab and outside-click
  dismissal; programmatic opening without a serial may not acquire a grab.
  Provide an explicit close action for that case. Escape closes the popup when
  it receives keyboard input.
- Dismissal invokes `actions` with
  `{type="popup_closed", id="menu", reason="escape"|"closed"}`.
  Native dismissal, including outside-click dismissal, reports `"closed"`;
  the compositor does not provide a more specific reason.
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
- Content and geometry updates reuse the child surface; geometry changes use
  the native popup positioner. Moving a popup to a different declaring layer
  creates a new child. List animation state, hover, and
  measurements remain scoped to its owner. Popup open/close lifecycle
   transitions, automatic widget anchoring/flipping, and nested popups are not
   supported by this app-popup contract. Legacy bar-widget popups have their own
   [lifecycle contract](lua-api.md#popup-openclose-transitions).

See `examples/apps/popup/app.lua` for a runnable counter/menu example.

Migration: older framework popups used output-relative `x`/`y` on a fullscreen
overlay. Update custom Lua to parent-relative positions; do not add output
origins or bar offsets. `riced.widget_popup` computes parent-relative positions
for embedded bar widgets automatically. Installed Lua files are not overwritten.

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

Run `riced -c example` from the repository root for the personal frontend snapshot
in `config/riced/example/`. It uses a plain `shell.lua` app entry, Lua layers,
and `data/bars/`, not the shared daemon builder. Its README lists machine-specific
settings and differences from the latest bundled frontend.

## Related APIs and current limits

The [structured persistence API](persistence.md) provides `riced.file()` with
cached edits, managed reload/save operations, conflict detection, and coalesced
file watching. The [experimental Rust extension SDK](extensions.md) provides
versioned registration, cached service snapshots, and managed service commands
for compiled-in Cargo crates under `extensions/<name>/`. Extensions are enabled
in `data/config.lua`, and `riced build` generates the registration entry point plus
a locked Cargo package under `.riced/`, so adding an extension no longer edits
framework source. The palette extension demonstrates this owned-data interface
with explicit Lua-controlled palette application. Compiled-in extensions require
rebuilding/restarting. Supervised process services now provide lazy executable
startup, capability negotiation, bounded commands, cancellation/deadlines,
crash detection, and restart-on-next-request through the same Lua service API.
See [process configuration and limits](extensions.md#supervised-executable-services).
Rust dynamic-library loading is
intentionally not part of the initial design.

Ordinary app windows are supported by `iced.window`. App popups still lack
automatic anchoring/flipping, lifecycle transitions, and nesting. Successful
source reloads reset app state; explicit restoration must use app-owned data.
