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
| `app:on_action(key[, value])` | Optional. Receives action keys from buttons, popup rows, and `on_press` targets.                                                                                                                         |
| `app:on_press()`              | Optional (widgets). Bare click action when no `popup()` opens.                                                                                                                                           |
| `app:transitions()`           | Optional. `{ add, remove, displaced }` motion overrides; see Transitions.                                                                                                                                |

`app.defaults` declares `interval`, `size`, and custom `props` (see Widget properties).
Before every `view()`/`popup()`/`on_action()`, the engine sets `self.props` to the
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
`app:on_press()`, and `app:on_action(key[, value])` work on the placed widget.
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

Open **Settings → Desktop → Place rectangle**, move to the desired display,
then click to place. Right-click or Escape cancels. Capture surfaces show the
desktop above ordinary windows while positioning; Settings stays intact below.
Successful drops save one `[[desktop]]` entry pinned to the clicked display.
Its top-left corner follows the cursor. Removing or moving placements is
available in the same Settings page. Unknown placement fields and invalid
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

Shared layout setters on every node: `:anchor()`, `:margin()`, `:on_press(key)`.

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

Clicking calls the owning placement's `app:on_action(key)` directly; a missing
`on_action` is a silent no-op. Action keys are opaque strings — Lua functions
cannot cross the VM boundary (trees are owned data; functions are discarded at
decode). A button without a key is inert.

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
- Optional per-item `callback` in any slot runs once when that transition completes, receiving the captured item; its return value flows through the action parser like `on_action`. No callback means silent ghost cleanup. Re-entering cancels a pending exit; reloads and teardown invalidate old callbacks.

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

Command constructors — **return** their result from `on_action` or a transition
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
| `system`        | `cpu_usage` (%), `cpu_count`, `mem_used`/`mem_total` (bytes), `mem_usage` (%), `gpu_usage` (% or nil)                                         |
| `theme`         | Live palette hex pairs: `primary`, `on_primary`, `secondary`, `surface`, `background`, `success`, `warning`, `error`, plus `on_*` text colors |
| `notifications` | Retained history, newest-first: `{ id, app, title, body, urgency, has_image, popup_visible, closed }` — metadata only                         |
| `wayland`       | `outputs` (`name`, `x`, `y`, `w`, `h`), `workspaces` (`name`, `monitor`, `active`), `toplevels` (`app_id`, `title`); empty where unsupported  |
| `bar`           | `bar.output` connector name (nil while unknown; nil in notification scenes)                                                                   |

Scene popup props (`props.notifications`, newest-first, output-filtered) carry
richer items: `{ id, app, title, body, icon, urgency, actions = {{key, label}}, has_image, image }`.
`image` is an opaque token for `iced.image()`; pixels never serialize through Lua.

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
