# Framework apps (experimental)

Riced can host opt-in Lua applications as native Wayland layer surfaces.
Existing bars, desktop widgets, notifications, and overlays remain supported.
This is the first framework milestone, not yet a stable extension SDK.

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
can still block the UI. Managed asynchronous tasks are a later milestone.
Do not perform blocking work in views or assume hook side effects can be rolled
back. Rust releases owned resources even if an unmount hook errors; process
termination is not guaranteed to run Lua cleanup hooks.

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

The host polls app-local Lua sources and shared definitions. A changed source
loads in a fresh VM, mounts, and validates its first layer view before replacing
the active runtime. Syntax, mount, and decode failures preserve the previous
surface. Corrected sources are retried. Successful reload resets app state;
persistent state migration is not implemented yet.

See `examples/framework/` for a Lua bar with a local component, clock, and
independent click counter on each output. Copy its `apps/bar/` directory into
your project and merge the example `apps` entry into your existing config.

## Subsequent milestones

Managed tasks and unified service contracts come next, followed by a versioned
Rust extension SDK and standard Cargo crates under `extensions/<name>/`.
Compiled-in extensions will require rebuilding/restarting; supervised process
extensions will follow for independently restartable service workloads. Rust
dynamic-library loading is intentionally not part of the initial design.
