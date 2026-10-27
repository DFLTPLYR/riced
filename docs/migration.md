# Migration: Lua-authored UI and plugin services

## Current default: restored shell flow with Lua IPC

The installed `root.lua` loads command handlers from `state.lua` and leaves the
original Rust bar/desktop hosts enabled. It no longer substitutes the demo
Settings-button/clock bar or the simplified background menu for the existing
slot editor, widget popups, desktop selection and drag interactions. The
`layers/` templates remain optional examples. Notifications and Settings keep
their existing lifecycle and Apply/Discard behavior.

`riced ipc settings` and the desktop Settings menu share the root's command
handler. `riced ipc add --anchor EDGE [--output NAME] [--thickness PIXELS]`
and `riced ipc remove --anchor EDGE [--output NAME]` use the native bar lifecycle,
with removal closing every surface owned by the recipe, including other outputs.
Existing user-authored project files are still preserved during initialization.
The remaining Lua ownership migration must retain these interactions before
replacing their hosts.

This tracks moving the remaining Rust-authored shell UI into Lua and
extracting optional hardcoded services into configurable plugins.

Scope: Lua owns screen layouts, presentation, and UI interaction state.
Plugins provide optional data and integrations. Core Rust keeps rendering
(Iced), native surfaces, input routing, plugin lifecycle, and privileged
operations. **Removing Iced itself is out of scope**; it would require a
replacement renderer and is a separate project.

## Layer ownership

| Layer | Owns |
| --- | --- |
| Lua | Widget trees, screen composition, hover/press presentation, editors |
| Plugins | Optional data providers and integrations behind the extension SDK |
| Core | Rendering, native surfaces, output identity, input routing, persistence, image decode/bake, plugin lifecycle, validated host actions |

Today the visual content of the bar, popups, desktop overlays, and
notifications is already Lua-composed. Rust owns the hosts, the surface
lifecycle, and — except for notifications — the screen shells. Settings is the
largest remaining Rust-only presentation layer.

## Screen inventory

| Area | Rust files | Replacement | Notes |
| --- | --- | --- | --- |
| Bar / slots | `src/shell/screens/top/` (`mod.rs`, `render.rs`, `edit.rs`, `animation.rs`, `scheduling.rs`, `runtime.rs`, `input.rs`) | Lua layer root + slot/grid composition | No Lua slot model or bar persistence yet |
| Placement popups | `src/shell/screens/popup.rs`, `popup/sizing.rs`, `popup/surface.rs` | Framework app popups + lifecycle | Needs anchoring/flipping/transitions/non-modal |
| Background / desktop | `src/shell/screens/background.rs` | Lua scene + composables | Context menu and selection are already Lua composables with a fixed host contract |
| Desktop widgets | `src/shell/desktop_widgets.rs` | Already Lua content; port the Desktop Settings page | Drag/place host is native |
| Notifications | `src/shell/screens/notification.rs`, `src/shell/overlay.rs` | Already a Lua scene | Needs Lua-side creation/hosting |
| Settings | `src/shell/screens/setting/` (`mod.rs`, `pages.rs`, `panel.rs`, `placement.rs`, `editors.rs`, `components.rs`, `context_menu.rs`, `wallpaper.rs`) | Lua pages + shared editor components | Largest Rust-only presentation; keep bake/validate/persist native |

## Service / plugin inventory

| Service | Topics | Mode | Extraction | Blockers |
| --- | --- | --- | --- | --- |
| `theme_gen` (`src/theme_gen/`) | none (CLI/settings) | command | Medium (best first) | Writes files; needs wallpaper pixels |
| `system` / `gpu` (`src/services/system.rs`, `gpu.rs`) | `system` | read-only poll | Easy–Medium | Host sampling cadence and `sampled` clock |
| `theme` snapshot (`src/theme/`) | `theme` | read-only poll | Easy–Medium | `ActiveTheme`/`style.rs` paint host chrome |
| `notifications` snapshot (`src/services/notifications.rs`, `src/notify/`) | `notifications` | event | Medium–Hard | Owns D-Bus name; pushes into shell |
| `wayland` (`src/services/wayland.rs`, `toplevels.rs`, `workspaces.rs`) | `wayland`, `wayland.*` | event | Hard | Needs listener/push; in-place list identity |
| `tray` (`src/services/tray.rs`, `src/tray/`) | `systemtray`, `tray` cmds | event + commands | Very hard | D-Bus name, event stream, opaque images |
| `wallpaper` (`src/wallpaper/`) | none (settings) | long-running + commands | Hard | Pixels cannot cross JSON; fs side effects |

`src/services/{registry,changes,state}.rs` and `src/services/bar.rs` are host
plumbing or per-surface context, not plugin candidates.

## Lua API gaps

- **Bar:** slot grid (`BarEvent::Slots`/`SlotAlign`/`SlotPadding`/`SlotSpacing`/
  `WidgetLayout`/`WidgetProp`) and persisted bar config (`TopEvent::Sow`/`Remove`).
- **Popups:** automatic anchoring/flipping, open/close lifecycle transitions,
  nested and non-modal popups.
- **Desktop:** a generic Lua overlay/selection primitive and an extensible
  context-menu contract.
- **Notifications:** Lua-side creation and popup hosting.
- **Settings editors:** spin box (`src/ui/widgets/spin_box.rs`), color/swatch
  row, file picker for legacy widgets, image previews from paths, tabs/pages
  navigation; `iced.image_handle` is undocumented.
- **Host actions:** wallpaper apply/bake, config persistence, theme generation,
  output/bar lifecycle.

## Phases

1. **Audit and boundaries** — this document. *(done)*
2. **Service plugin foundation** — consistent snapshot/command/error/availability
   contracts; event-driven updates for long-lived services; define startup,
   reconnection, cancellation, shutdown, and bounded delivery; keep plugins free
   of Lua and mutable shell-state references.
   - *Progress:* compiled-in services can now publish unsolicited snapshots via
     `Service::refresh`, polled by the host and republished to subscriptions.
     Supervised process services use the same model through protocol v2's
     id-matched `refresh`/`snapshot` exchange, polled only while the service is
     being read.
3. **Reference plugins** — palette/theme-generation and Wayland/compositor
   examples, each with config, build instructions, and contract tests.
   - *Progress:* `extensions/theme-gen` provides the `theme_gen` service
     (seed → reshell scheme JSON + template variables) and owns the pure scheme
     logic the host's `theme_gen` module re-exports, so there is one
     implementation. Wallpaper sampling and file writes remain in the host.
     `examples/apps/theme_gen/` is the sample card.
   - *Progress:* the example project also mirrors the shell-level configuration
     surface a real setup uses (`theme`, `animation`, `interaction`,
     `notifications`, and commented wallpaper/desktop templates), a three-slot
     bar layout, and an `editors` card that drives `iced.masonry` from
     `iced.spin_box`. Contract tests load the committed apps and config.
   - *Progress:* `riced extension build [NAME]` compiles the extension crates a
     project enables and `riced extension update` compiles them and rebuilds the
     generated shell. A process entry's `path` names its crate directory
     (default `extensions/<name>`); `examples/extensions/hello` is a complete
     buildable process extension.
   - *Progress:* framework layers now support host slots (`iced.native`), so a
     Lua background embeds the native desktop-widget host, and
     `shell.add_layer`/`shell.remove_layer` add and drop app layers at runtime
     with persistence. The layer set is no longer fixed by `config.lua`.
   - *Progress:* an app declares a *list* of layers, each with its own surface,
     list state and measurement, so one module can own a background, a bar and
     an overlay. Layer ids are unique per list; a dropped layer closes only its
     surface. `riced.layer("layers/x.lua", props)` composes a layer from its own
     file, and the example owns its bar list in a managed document instead of
     asking the host for a layer: `shell.add_layer`/`remove_layer` are gone.
     Binding Settings to that document (S2) is still open — settings pages have
     no managed file I/O yet.
   - *Progress:* `extensions/wayland-toplevels` and
     `extensions/wayland-workspaces` own the `ext-foreign-toplevel-list-v1` and
     `ext-workspace-v1` listeners and expose the `toplevels` and `workspaces`
     services; the host's `services::toplevels`/`services::workspaces` re-export
     them, so each core `wayland.*` table and its service share one
     implementation. Outputs are still core-only, and neither extension is in
     the default registry (each adds a Wayland connection when enabled).
4. **Lua UI API** — close the gaps above while preserving budgets, hot reload,
   last-good views, and stale-callback protection.
   - *Progress:* `iced.spin_box(value, min, max)` is now exposed (the native
     `[-] type-in [+]` stepper Settings uses), and `iced.image_handle` is
     documented. `scripts/components/06-editors.lua` now supplies pure Lua
     swatch/color rows, path/token image previews, file-action rows, and
     stateless page navigation. Framework apps use `self:pick_file` from
     their action handler; legacy widgets still lack a managed picker.
     These are presentation building blocks, not a completed Settings port.
5. **Settings to Lua** — shared editor components, then theme/animation, panel/
   placement, context menu, and wallpaper pages. Keep bake/validate/persist native.
   - *Progress:* the native Settings window now renders its Animation page from
     `scripts/settings/animation.lua`. Rust supplies the speed/duration snapshot,
     caches owned descriptions per window, and maps an allowlist of actions to
     the existing config-patch/save pipeline. The replaced Rust page layout has
     been removed. This bundled page is embedded at build time (not a project
     app and not hot-reloaded yet).
   - *Progress:* Theme Settings now renders `scripts/settings/theme.lua` with
     mode controls, palette previews/swatches, and icon-theme cards using the
     exposed `iced.masonry` primitive. Rust retains asynchronous icon loading,
     metadata snapshots, entry-scoped image tokens, and allowlisted config
     patches. The old `setting/pages.rs` layouts have been removed. Theme and
     Animation remain bundled pages in the native Settings window, not yet
     project-owned framework apps or hot-reloaded sources. Other pages remain
     native.
   - *Progress:* the Panel page's per-placement editor now renders
     `scripts/settings/placement.lua`. Interval, text size, scalar property
     controls, choice buttons, inheritance captions, and resets are Lua-owned.
     Native adapters validate typed actions and numeric bounds, preserve
     placement identity, and use the existing sparse persistence path. Invalid
     numeric schemas are shown read-only. The old Rust placement controls have
     been removed.
   - *Progress:* the Panel page's bar-level controls now render
     `scripts/settings/panel_bar.lua` in two halves (layout, then appearance)
     around the native arrangement preview and the Lua placement editor:
     length, thickness, opacity presets, slot count/padding/spacing, slot and
     alignment pickers, floating, margins, corner rounding, and bar removal.
     The native preview remains the only drag-and-drop surface, and the old
     `plant_slider_row` helper has been deleted.
   - *Progress:* the Components page now renders
     `scripts/settings/components.lua`: component/property pickers, the Lua
     value input, apply/reset actions, the saved-defaults list, and validation
     errors. Rust still owns Lua-literal parsing, `ComponentDefaults`
     validation, and persistence. The page deliberately runs with empty
     component defaults so editing a default does not restyle the editor
     itself. Absent snapshot fields are omitted rather than sent as `null`,
     because JSON `null` reaches Lua as a truthy light userdata.
   - *Progress:* `iced.native(id)` is now a description node that the realizing
     host replaces with a host-owned element (`build_node_with_native` /
     `realize_with_native`); a description without a resolver errors instead of
     silently dropping the slot. The Desktop page renders
     `scripts/settings/desktop.lua` with hold-duration controls, the preview
     display picker, and both widget grids, embedding the live inert previews
     as native slots. Discovery, preview identity/pickup, and every config
     patch remain native; the old Rust `settings()` page has been removed.
     `native` is excluded from the Components defaults editor because it is a
     host slot, not a user-configurable component.
   - *Progress:* the Context Menu page renders
     `scripts/settings/context_menu.lua`; the two composable source editors are
     Lua and typing still maps to the existing `ComposableSource` patch. The
     old Rust `context_menu::view` and the now-unused `editors::prop_row`
     helper have been removed.
   - *Progress:* the Wallpaper page renders `scripts/settings/wallpaper.lua`:
     mode buttons, the Add/Discard/Apply header, the free-form image picker and
     placement fields, and the per-monitor picker, content-fit buttons, and
     clear/choose actions. The interactive map canvas stays native behind an
     `iced.native("wallpaper-map")` slot, and Rust still owns draft state,
     preview sources, baking, and persistence. Deleting the last native page
     controls also removed `setting/editors.rs` and the now-unused native
     `ui/widgets/spin_box.rs` (the Lua `iced.spin_box` component is separate).
     Every Settings page is now Lua-authored.
   - *Progress:* Settings pages are project-owned. Bundled copies are installed
     once into `settings/` (never overwriting an existing directory, and
     deletions stay deleted, like `widgets/` and `components/`). Each page
     adapter loads its project copy through `setting/sources.rs` and falls back
     to the embedded source when the copy is missing or fails to decode, so a
     bad edit cannot take Settings down. Source stamps are part of every page
     cache key, so saving a page reloads it. The Settings *window* chrome now
     renders `settings/shell.lua` through the same project-copy resolution: Lua
     owns the page catalog, nav order, labels, descriptions, and hints, and Rust
     maps `settings:page:<id>` actions to validated page ids and fills the
     `settings-page` native slot with the selected page host. Pages are not yet
     ordinary framework apps with their own surfaces. `examples/` commits one
     project-owned page (`settings/animation.lua`) with a contract test and
     ignores the other runtime-seeded libraries.
6. **Remaining shell presentation** — desktop menus/overlays, notifications,
   popups, then bar/slot composition.
7. **Wire the reference shell** through configuration; show unavailable states
   for absent plugins; never start services from rendering.
8. **Verify** — tests plus manual parity (see below).
9. **Remove superseded code** — delete replaced Rust screens and hardcoded
   services, drop dead dependencies, update docs, run `graphify update .`.

## Verification

### Default frontend startup

Plain daemon startup now installs `scripts/frontend/` into the default project
when neither Lua nor legacy TOML configuration exists. `apps/shell/app.lua`
owns the background, repeatable bars and a Lua layout editor. The editor and
layer list use the same per-output managed JSON document; bar identities survive
edits. Existing projects are preserved, not automatically converted. The starter
does not reproduce the legacy seven-widget bar, wallpaper baking/map workflow,
or native Settings pages. Those remain parity work before deleting old hosts.

Installation uses atomic no-clobber file publication, an installation lock,
a pending journal and a versioned completion marker. `riced init --repair`
restores missing files only, without overwriting edits. Explicit projects skip
automatic default installation. Tests exercise startup and repair with private
XDG directories and render the installed frontend through the real app host.

The installed frontend is a single file, `apps/root.lua`, with its layer
modules in the shared project `layers/`; `riced.layer` resolves the app's
`components/` first, then the project directory, then the shared library. A `TopConfig` cannot be sent to Lua directly: it serializes through the file
format, which drops default-valued fields, so the app saw a nil `anchor` and the
whole view failed. The host now projects an explicit, complete bar shape. Bar
removal and addition edit `config.bar` — the list Settings → Panel already
owns — through `shell.add_bar`/`shell.remove_bar`, and `bar_host = false` stops
the native bar host from drawing those same edges. **Settings** always opens the
riced window, so there is exactly one Settings.

The installed frontend renders the wallpaper published by Wallpaper Settings
through an `iced.native("wallpaper")` slot, so Apply in Settings is visible on a
Lua background without moving baking, shaders, theme regeneration, or the pixel
cache out of Rust. That slot is empty until a bake succeeds, so the layer also
draws `props.wallpaper` — the configured source for its output — underneath it;
a failed or pending bake shows a wallpaper instead of a blank screen. Bar removal is a per-bar `popup` opened by a right-click on
that bar; the action event carries `event.layer`, the id of the layer it came
from, because every layer of one app shares a single Lua state.

- Lua screen descriptions and action contracts.
- Plugin snapshots, commands, availability, lifecycle, cancellation, crashes,
  reconnection, and event ordering.
- Settings persistence and wallpaper apply/reset.
- Popup sizing, focus, input, scrolling, animations.
- Hot reload and invalid-Lua recovery.
- Multi-monitor changes.
- Missing plugins and unsupported compositor capabilities.
- Manual parity plus startup/event/render cost checks.

## Open decisions

- How the extension SDK should expose long-lived event streams (push from a
  service) versus the current host-poll `refresh` model.
- Whether optional Wayland services stay built-in behind a config toggle or
  move behind the plugin contract entirely.
- The Lua slot/grid model that replaces the native bar layout editor.
- How a push-only process service should start: supervisors currently launch a
  worker lazily on the first command, so a command-less service (such as
  `toplevels`) is compiled-in only. Either the host starts workers on first
  read, or such services declare that they may start without a command.
