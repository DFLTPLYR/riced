# Lua API reference

Every Lua global available to widgets, popups, overlay scenes, and composables.
Rust owns delivery, policy, and validation; Lua owns presentation. ◪

## App contract

Each script returns an app table. `app:view()` is required.

```lua
local app = {}
function app:view()
    return iced.row({ iced.icon("cpu"), iced.text("42%") })
end
return app
```

| Method                        | Purpose                                                                                                                                                                                                  |
| ----------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `app:view(props)`             | Required. Returns text (numbers/booleans coerce, `nil` is empty) or an `iced.*` tree. Widgets, scenes, and composables receive the resolved `props` table, also available as `self.props`.               |
| `app:popup()`                 | Optional (widgets). Returns a popup descriptor: `{ text }`, `{ text, width, height }`, `{ items = {{label, action}} }`, and/or `{ ui = <tree>, width, height }`. No text, tree, or items means no popup. |
| `app:actions(events)`          | Optional. Receives one owned event table per invocation: control actions, pointer gestures, and delivered keyboard events. |
| `app:transitions()`           | Optional. `{ add, remove, displaced }` motion overrides; see Transitions.                                                                                                                                |

`app.defaults` declares `interval`, `size`, and custom `props` (see Widget properties).
Before every `view()`/`popup()`/`actions()`, the engine sets `self.props` to the
resolved table — read per-instance values there, never from `app.defaults` directly.
It also sets `self.hovered` (`true` while the pointer is over that placement or
component, else `false`); see Hover state.

## Desktop widgets and placement

`widgets/<name>.desktop.lua` declares a desktop app. The `.desktop.lua` suffix
classifies placement separately from bar widgets; `rectangle.desktop.lua` is named
`rectangle` in Settings and configuration. Dock/bar and desktop widgets use the
same widget runtime, Lua libraries (`os.date`, `io`, and native shell access),
service tables, resolved props, and action entry points. Each placement/output
has an independent instance. Views may return text or `iced.*` trees.

```lua
local app = { defaults = { width = 240, height = 160, radius = 12 } }
function app:view(props)
    return iced.container(iced.space())
        :width(props.width):height(props.height):radius(props.radius)
        :background(self.hovered and theme.primary or theme.surface)
end
return app
```

`props` and `self.props` contain Lua defaults, overlaid with placement `props`,
then host-owned `x`, `y`, optional size overrides, and `output` (name, global
logical origin, logical width/height). Coordinates are **output-local logical
pixels**, independent of monitor scale. Optional width/height must resolve to
finite, positive numbers at most 32768; absent dimensions use intrinsic sizing.
Lua owns styling and default dimensions. `self.hovered`, bare-click
`app:actions(events)` works on the placed widget.
Like dock/bar widgets, desktop widgets re-run `view(props)` on
`app.defaults.interval` (default 1 second, minimum 0.25 seconds), as well as on
placement, theme, source, hover, or action changes. Return `os.date(...)` from
the view for a live clock. Desktop surfaces currently have no popup-window host.

```toml
[[desktop]]
widget = "rectangle"
output = "DP-1"       # omit or "" to display on every output
x = 400
y = 300
# width = 320        # omit or 0 to inherit the Lua default
# height = 200
# z = 1              # higher values paint in front
props = { radius = 16 }
```

Open **Settings → Desktop** and click inside a widget preview, move to the desired
display, then click to place. The page renders each discovered desktop widget's
actual Lua tree with its defaults; placed widgets use their saved props and size
overrides. The preview-display selector supplies output context to unpinned
widgets. Preview instances are isolated from live desktop instances, and embedded
controls do not execute actions. Right-click or Escape cancels. Capture surfaces show the
desktop above ordinary windows while positioning; Settings stays intact below.
Successful drops save one `[[desktop]]` entry pinned to the clicked display.
The point originally pressed inside the widget stays attached to the cursor;
scaled previews map that point back into native logical pixels, excluding card
padding and letterboxing. The ghost and saved origin both use `cursor - grab`.
Explicit **Place** and **Reposition** buttons use the top-left corner as their
fallback anchor. **Reposition** on the same Settings
page picks up an existing placement; the next click saves its output and position
while preserving identity, dimensions, props, and stacking order. Escape or
right-click cancels without changing the saved placement. Unknown placement fields and invalid
coordinates/dimensions fail configuration parsing. Disconnected-output entries
are retained and resume displaying when that output returns.

Source edits hot-reload; invalid replacements keep the last working view.
The bundled rectangle is seeded without overwriting existing files. The
`.desktop-widgets-initialized` marker prevents subsequent starts from restoring
an intentionally deleted example.

## `iced.*` constructors

The sole constructor namespace. Setters chain and return the node; each setter
writes its field, last write wins. Wrong-type values and unknown setters fail
at decode with the field named.

| Constructor                                                    | Arguments                                                                                                  | Setters                                                                                                                                           |
| -------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `iced.text(s)`                                                 | text                                                                                                       | `size`, `width`, `height`, `wrap`, `color`, `elide`, `elide_mode`                                                                                 |
| `iced.icon(name)`                                              | Lucide name, case/separator-insensitive, plus aliases (`mem`, `vol`, `up`); unknown names render literally | `color`                                                                                                                                           |
| `iced.row({...}[, spacing])` / `iced.column({...}[, spacing])` | children array                                                                                             | `spacing`, `width`, `height`                                                                                                                      |
| `iced.button(label, action)`                                   | label, action key                                                                                          | `action`, `width`, `height`, `padding`, `radius`, `background`, `color`                                                                           |
| `iced.progress(v[, width])`                                    | 0.0–1.0 clamped, default width 120                                                                         | `width`, `height` (thickness), `color` (fill), `background` (track)                                                                               |
| `iced.spinner()`                                               | —                                                                                                          | —                                                                                                                                                 |
| `iced.separator()`                                             | —                                                                                                          | `height`, `color`                                                                                                                                 |
| `iced.container(child)`                                        | child                                                                                                      | `width`, `height`, `padding`, `background`, `radius`, `border`, `border_width` (`:border()` implies 1px; bare width without color paints nothing) |
| `iced.scrollable(child)`                                       | child                                                                                                      | `width`, `height`, `scrollbar`                                                                                                                    |
| `iced.space()`                                                 | —                                                                                                          | `width`, `height`                                                                                                                                 |
| `iced.image(path)`                                             | file path                                                                                                  | `width`, `height`                                                                                                                                 |
| `iced.listview(data)`                                          | data array                                                                                                 | `id` (required), `key` (required field name), `delegate`, `axis`, `pitch`, `spacing`, `width`, `height`, `onEntered`, `onExit`, `onDisplaced`     |
| `iced.checkbox(label, checked)`                                | —                                                                                                          | `checked`, `on_toggle`, `width`, `size`, `spacing`                                                                                                |
| `iced.toggler(label, enabled)`                                 | —                                                                                                          | `enabled`, `on_toggle`, `width`, `size`, `spacing`                                                                                                |
| `iced.slider(min, max, value)`                                 | finite `min < max`, positive `step`                                                                        | `value`, `step`, `on_change`, `on_release`, `width`, `height`                                                                                     |
| `iced.text_input(placeholder, value)`                          | —                                                                                                          | `value`, `on_input`, `on_submit`, `width`, `size`, `padding`, `secure`                                                                            |
| `iced.pick_list(options, selected)`                            | unique-string options; selected nil or member                                                              | `selected`, `on_select`, `width`, `placeholder`, `padding`, `size`                                                                                |
| `iced.radio(label, value, selected)`                           | string value/selection                                                                                     | `selected`, `on_select`, `width`, `size`, `spacing`                                                                                               |
| `iced.canvas(drawing)`                                         | inert drawing table (see below)                                                                            | `width`, `height`                                                                                                                                 |

Shared layout setters on every node: `:anchor()`, `:margin()`, `:on_press(key)`.

### Canvas

`iced.canvas(drawing)` paints an inert, click-through drawing in canvas-local
logical pixels. `drawing` is `{ rects = {...}, lines = {...} }`: rectangles fill
first (backgrounds, grids, bands), then lines stroke on top. Each rect is
`{ x, y, width, height, color }` (width/height non-negative); each line is
`{ points = {{x, y}, ...}, color[, width = 1] }` (needs ≥ 1 point; two or more
paint). Colors take `"#rgb"` / `"#rrggbb"` / `"#rrggbbaa"` or `{r, g, b[, a]}`
0–1 tables. Coordinates are not scaled: the canvas clips, so give graphs an
explicit `:width(n):height(n)` rather than `"fill"`.

The bundled `30-graph.lua` component wraps this into `iced.graph{ series = {...} }`
(`series = {{ values = {…0–100…}, color, width }, ...}` plus `window`, `grid`,
`grid_color`); the `usage` / `usage.desktop` widgets use it for CPU/RAM/GPU.

### Sizes

Numbers are px. `"fill"` takes all remaining space; `"fill:n"` (positive integer
`n`, max 65535) takes `n` shares of it (`"fill"` ≡ `"fill:1"`). `"shrink"` and
`"auto"` (alias for `"shrink"`) fit content. Unset sizes mean shrink, except
progress width (`Fixed(120)`). Fill only expands when the parent offers space.
Also available as `size.fill`, `size.shrink`, `size.auto`; unknown `size.*`
keys raise. Invalid size strings raise naming the field.

### Text

- `:wrap("none" | "word" | "glyph" | "word-or-glyph")`, default word wrapping.
- `:elide(n)` keeps at most `n` characters including `…`; `:elide_mode("head" | "middle" | "tail")`, default tail.
- `:color()`/`:background()`/`:border()` take `"#rgb"` / `"#rrggbb"` / `"#rrggbbaa"` or `{r, g, b[, a]}` 0–1 tables.

### Buttons and actions

```lua
iced.button("{icon:x}", "dismiss:" .. item.id):width(40)
-- equivalent chained form; last write wins:
iced.button("{icon:x}"):action("dismiss:" .. item.id):width(40)
```

Clicking calls the owning placement's `app:actions({type="action", key=key})` directly; a missing
`actions` method is a silent no-op. Action keys are opaque strings — Lua functions
cannot cross the VM boundary (trees are owned data; functions are discarded at
decode). A button without a key is inert.

### Events and repositioning

`app:actions(events)` receives **one event table**, not a batch:

| `events.type` | Fields |
| --- | --- |
| `action` | `key`, optional `value` (boolean, number, or string) from a control or popup row |
| `press`, `release`, `click`, `hold` | `button` (`left`, `right`, `middle`, or another button name), `x`, `y` when available |
| `drag`, `drop` | Desktop left-hold pickup and release; `button`, `x`, `y` when available |
| `move`, `enter`, `exit` | `x`, `y` when available |
| `key` | `key` (character or named key), `state` (`pressed`/`released`), `modifiers = {shift, control, alt, super}` |

Pointer coordinates are placement-local logical pixels. A hold fires once after
the configured duration and suppresses the click. Leaving the placement cancels its pending gesture.
Child controls consume their own input; a button sends its control event without
a second bare click. Unconsumed gestures go to `actions`. Defining `popup()`
provides its content; request opening explicitly from `actions`:

```lua
function app:actions(events)
    if events.type == "click" and events.button == "left" then
        self:popup()
    end
end
```

Inside `actions`, `self:popup()` requests the bar popup; outside `actions`,
the host calls the same method to build its content. The popup is centered on
the reporting widget's measured bounds along the bar, at the bar edge.
Requesting it again toggles that placement's popup closed; requesting another
placement replaces the open popup. Desktop placements have no popup-window host.
Keyboard events require a host that receives keyboard input and a pointer over
the placement; passive bar/desktop surfaces do not request keyboard focus.
The native Lua demo supports delivered keyboard events.

On desktop placements, a left-button hold emits `drag` instead of `hold`.
Calling `self.drag()` from that event picks up the placement at the original
press point. Move with the button held, then release: the originating Lua instance
receives `drop`, even outside the widget or on another output. Call `self.drop()`
to save the position. Declining the drop or raising an error cancels it. Both
events carry `button="left"` and widget-local coordinates; drop coordinates refer
to the grab point on the moving snapshot. These host methods also accept colon
syntax, require their corresponding event, and are reserved for desktop instances.
Right-click cancels without changing the saved placement. Escape also cancels
when the source receives keyboard input. Held-drag previews neither take pointer
input nor request keyboard focus: the original surface keeps the button grab
until its matching release. Preview-surface, other-button, and stale-session
releases cannot end that gesture.

The ghost retains the source layout and grab point across outputs. Motion from
the initiating Wayland surface is mapped through logical output geometry during
the implicit grab. Reloaded/removed sources, changed placements, and disconnected
outputs invalidate stale drops. Settings previews and explicit Place/Reposition
buttons retain their click-to-pick-up, click-to-place interaction.

Configure hold timing in **Settings → Desktop → Hold duration**, or in `config.toml`:

```toml
[interaction]
hold = "medium" # fast = 250ms, medium = 500ms (default), slow = 1000ms
# hold = 750    # custom milliseconds, integer 1..=60000
```

The setting applies to widget holds on both desktop placements and bars.

```lua
function app:actions(events)
    if events.type == "drag" then
        self.drag()
    elseif events.type == "drop" then
        self.drop()
    elseif events.type == "action" and events.key == "enabled" then
        self.enabled = events.value
    end
end
```

`app:on_action()`, `app:on_press()`, and `app:handler()` are rejected at load.
Migrate installed scripts explicitly; seeds never overwrite user-owned files.
`self.reposition()` was removed; use the `drag`/`drop` lifecycle above.
Tree setters such as `:on_press(key)` remain valid action bindings.

### Scrollable

```lua
iced.scrollable(rows):width("fill"):height(320):scrollbar("hidden")
```

Constrain `:height()` so taller content scrolls. `:scrollbar("hidden" | "visible")`,
case-insensitive, default visible; unknown values raise. Hiding keeps
wheel/touchpad scrolling.

### Listview and transitions

```lua
iced.listview(items):id("list"):key("id")
    :delegate(function(n) return iced.text(n.label) end)
    :onEntered({ x = { from = 200, to = 0 }, opacity = { from = 0, to = 1 }, duration = 250 })
    :onExit({ x = { to = -200 }, opacity = { to = 0 }, duration = 250 })
    :onDisplaced({ duration = 250 })
```

- `pitch` is the estimated row size plus spacing along the list axis (displaced math).
- `transitions()` returns `{ add, remove, displaced }` in the same shape; partial specs compose with defaults.
- Optional per-item `callback` in any slot runs once when that transition completes, receiving the captured item; its return value flows through the command parser like `actions`. No callback means silent ghost cleanup. Re-entering cancels a pending exit; reloads and teardown invalidate old callbacks.

### Components

```lua
iced.define("stat", function(props)
    return iced.row({ iced.icon(props.icon), iced.text(props.value) })
end)
-- call as iced.stat({...}), iced.use("stat", {...}), or iced["my-name"]({...})
```

`components/*.lua` (sorted) runs in every state after `iced` is built. Names
containing `-` must use `iced.use("name", props)` or `iced["name"](props)` —
`iced.my-name` parses as subtraction. Unknown component names and non-node
returns error naming the component. `iced.to_table(node)` exports a description
table; `iced.components.<name>` exposes `{ rust_type, arguments, setters }`.

## Hover state

Host-owned booleans, re-published before `view()` whenever they change. Lua holds
no live references: the engine derives the flags from iced enter/exit reports and
re-runs only the affected view. Moving within the same row never re-executes Lua.

| Flag           | Scope                                | Meaning                                     |
| -------------- | ------------------------------------ | ------------------------------------------- |
| `self.hovered` | widget placements, composable apps   | pointer is over this placement/instance     |
| `item.hovered` | every `listview` delegate invocation | pointer is over this row's `(list id, key)` |

```lua
:delegate(function(n)
    local card = riced.component("notification_card.lua", n)
    if n.hovered then card = card:background(theme.hover) end
    return card
end)
```

Row hover flows into nested components: `riced.component("card.lua", item)` forwards
`item.hovered` into the card's `self.hovered`, so cards stay self-contained:

```lua
function app:view(props)
    return iced.container(iced.text(props.title))
        :background(self.hovered and theme.primary or theme.surface)
end
```

Rules: absent means unhovered (an untracked list decodes `hovered` as `false`, so
`item.hovered` is always a safe boolean test). Exit ghosts are inert and never
hovered. Entering one row clears the rest of its surface (single pointer), which
also heals flags left stale by a closed window. No hover while untouched by the
pointer (touch/no-pointer yields all-`false`). Native button hover/pressed
styling stays as the default; read the flag only to style surrounding content.

## `riced.*`

| Function                                    | Purpose                                                                                                                             |
| ------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `riced.component(src[, overrides][, host])` | Render a `components/` module inline. Frame pattern: `riced.component(props.item_component.src, props.item_component.props, item)`. |
| `riced.invalidate()`                        | Mark the view dirty.                                                                                                                |

## `notification.*` (scenes, widgets, popups, callbacks)

Command constructors — **return** their result from `actions` or a transition
callback. Calling without returning does nothing.

| Function                    | Returns                  | Effect                                                                         |
| --------------------------- | ------------------------ | ------------------------------------------------------------------------------ |
| `notification.dismiss(id)`  | `{ dismiss = id }`       | Delete one retained record; emits D-Bus close reason 2 unless already expired. |
| `notification.dismissAll()` | `{ dismiss_all = true }` | Delete all retained records across outputs.                                    |

Raw equivalents `{ dismiss = id }`, `{ dismiss_all = true }`, and
`{ invoke = { id = id, key = key } }` (validated D-Bus action, then dismissed)
also work. Malformed commands are ignored.

## Service tables (republished before every entry)

| Global          | Fields                                                                                                                                        |
| --------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| `system`        | `cpu_usage` (%), `cpu_count`, `mem_used`/`mem_total` (bytes), `mem_usage` (%), `gpu_usage` (% or nil), `sampled` (host-clock seconds)          |
| `theme`         | Live palette hex pairs: `primary`, `on_primary`, `secondary`, `surface`, `background`, `success`, `warning`, `error`, plus `on_*` text colors |
| `notifications` | Retained history, newest-first: `{ id, app, title, body, urgency, has_image, popup_visible, closed }` — metadata only                         |
| `wayland`       | `outputs` (`name`, `x`, `y`, `w`, `h`), `workspaces` (`name`, `monitor`, `active`), `toplevels` (`app_id`, `title`); empty where unsupported  |
| `systemtray`    | `items`: registered StatusNotifier items, including icons, tooltips, and nested D-Bus menus; see below                                       |
| `bar`           | `bar.output` connector name (nil while unknown; nil in notification scenes)                                                                   |

`system.sampled` is the host-clock time (seconds) of the last reading. Gate
sample history on it rather than on render count: open popups redraw several
times per reading, so the `usage` graph appends one point per elapsed step.

Scene popup props (`props.notifications`, newest-first, output-filtered) carry
richer items: `{ id, app, title, body, icon, urgency, actions = {{key, label}}, has_image, image }`.
`image` is an opaque token for `iced.image()`; pixels never serialize through Lua.

### System tray

riced hosts `org.kde.StatusNotifierWatcher` on the session bus. Item registrations,
property signals, menu changes, and app exits update an event-driven cache; Lua
entries only read snapshots. If another watcher owns the name, riced logs the
conflict and leaves `systemtray.items` empty. It does not replace that watcher.

`systemtray.items` is an array in registration order. Each item has:

| Field | Meaning |
| --- | --- |
| `id` | Unique routing key (`bus-name/object/path`); pass this to every `tray.*` command |
| `app_id`, `title` | App-supplied identifier (not necessarily unique) and display title |
| `status` | SNI status, normally `Passive`, `Active`, or `NeedsAttention`; Lua decides visibility |
| `icon_name`, `icon_path` | App's icon name and resolved theme/absolute path (nil if unresolved) |
| `icon_pixmap` | Optional opaque image token decoded from SNI ARGB pixels; use `iced.image(token)` |
| `attention_icon`, `overlay_icon` | Optional opaque image tokens for attention/overlay icons |
| `tooltip_title`, `tooltip` | Tooltip title and description |
| `is_menu` | App requests menu behavior rather than ordinary activation |
| `has_menu`, `menu` | Whether a menu layout is available, and its root node (otherwise nil) |

Every menu node has `id`, `label`, `type` (`separator` or the app's normal type),
`enabled`, `visible`, `toggle_type`, `toggle_state` (-1/0/1), `checked`,
`icon_name`, optional `icon` (image token), `submenu`, and a `children` array.
Labels retain D-Bus menu mnemonic underscores; Lua owns how they are displayed.
Hidden and disabled entries remain in the snapshot so Lua can style them.
The host rejects delivery of clicks through hidden/disabled branches.

The separate **`tray`** global constructs commands. Return a command from
`actions(events)` or a transition completion callback; constructing one alone
does not perform I/O. All D-Bus calls run asynchronously.

```lua
return tray.activate(item.id)                 -- Activate(x, y), default 0, 0
return tray.secondary(item.id, x, y)          -- SecondaryActivate
return tray.context(item.id, x, y)            -- app-owned ContextMenu
return tray.scroll(item.id, 120, "vertical") -- vertical or horizontal only
return tray.open(item.id)                     -- AboutToShow(0) + placement popup
return tray.aboutToShow(item.id, entry.id)    -- prepare a submenu, no popup toggle
return tray.menuEvent(item.id, entry.id)      -- D-Bus menu "clicked" event
```

Coordinates are screen coordinates supplied to the app, not widget-local points.
`tray.open` uses the existing bar-placement popup host; implement `app:popup()`
to render the selected item's menu. It requests `AboutToShow` and refetches the
layout; open popup bodies refresh when the result arrives. Submenu preparation
and `LayoutUpdated`/`ItemsPropertiesUpdated` signals also refresh the cached tree.
Invalid command fields and unknown orientations are errors. Data (`systemtray`)
and constructors (`tray`) have separate names so snapshot publication never
overwrites action functions.

`scripts/widgets/systemtray.lua` is the bundled example: it hides passive items,
shows attention icons, and opens Lua-rendered menus with nested navigation,
separators, and toggle state. Items without a menu activate directly. The popup
also offers activation and the application's native context menu. Add the
`systemtray` widget to a bar in Settings. Fresh widget libraries receive the seed;
existing libraries can copy this file into their `widgets/` directory.

## Widget properties

```lua
local app = {
    defaults = {
        interval = 1.0, size = 13.0,
        props = { format = "%H:%M", show_seconds = false },
    },
    property_schema = {
        format = { label = "Time format", type = "string" },
        show_seconds = { label = "Show seconds", type = "boolean" },
    },
}
```

Slot placements override via `{ name = "clock", size = 16.0, props = {...} }`;
bare `"name"` inherits everything. `property_schema` fields (`label`, `type`,
`description`, `min`/`max`, `choices`) are optional; without it, controls infer
from default value types. Each placement gets an independent Lua state.

## Sandbox and budgets

- Widget/scene VMs: `string`/`table`/`math`/`os`/`io` with native shell (`os.execute`, `io.popen`); `require`, `dofile`, `loadfile`, `os.exit`/`os.remove`/`os.rename` are nil. Other app VMs have a narrower stdlib (no `os`/`io`).
- 64 MiB memory limit, ~200k instruction budget per entry; overruns error and the next entry recovers. `print()` goes to the daemon log.
- Cell text may embed `{icon:name}` placeholders for theme-aware Lucide icons.
