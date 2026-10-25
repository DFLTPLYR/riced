# Framework apps (experimental)

Riced can host opt-in Lua applications as native Wayland layer surfaces.
Existing bars, desktop widgets, notifications, and overlays remain supported.
The application and managed-resource APIs are experimental, not yet a stable
extension SDK.

## Project layout

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
    return iced.layer({
        id = "main",
        layer = "top",
        anchors = { top = true, left = true, right = true },
        height = props.height,
        exclusive_zone = props.height,
        keyboard = "none",
        input = "all",
        content = iced.text("Hello " .. props.output.name),
    })
end
return app
```

One layer root is supported per app/output instance. Return `nil` to hide it
without discarding application state. Layer roots cannot appear in widget trees.

| Field | Contract |
| --- | --- |
| `id` | Nonempty surface identity; defaults to `"main"`. Changing it recreates the native surface, not the Lua instance. |
| `layer` | `"background"`, `"bottom"`, `"top"` (default), or `"overlay"`. |
| `anchors` | Named booleans: `top`, `right`, `bottom`, `left`; all default false. |
| `width`, `height` | Integer logical pixels in `1..32768`. An omitted dimension stretches and requires both opposite anchors. |
| `exclusive_zone` | Integer `-1..32768`, default `0`; positive values reserve space according to Wayland layer-shell rules. |
| `margin` | Named integers `top`, `right`, `bottom`, `left`, default zero. |
| `keyboard` | `"none"` (default), `"on_demand"`, or `"exclusive"`. |
| `input` | `"all"` (default) or `"none"`; partial measured input regions are not exposed yet. |
| `pointer_events` | Boolean, default false. Deliver unconsumed layer-local pointer gestures to `actions`; child controls handle input first. |
| `content` | Required ordinary `iced.*` widget tree. |

Unknown fields and invalid geometry fail before native creation. Layout, focus,
layer, margins, input, and reserved space reconcile on the same native window.
Native UI realization/animation uses cached owned trees, never per-frame Lua.

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

See `examples/framework/` for a Lua bar with a local component, clock, and
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
file watching. A versioned Rust extension SDK, generic
service registration/commands, and standard Cargo crates under
`extensions/<name>/` follow the persistence milestone.
Compiled-in extensions will require rebuilding/restarting; supervised process
extensions will follow for independently restartable service workloads. Rust
dynamic-library loading is intentionally not part of the initial design.
