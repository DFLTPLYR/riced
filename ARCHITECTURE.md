# Architecture

## Module ownership

```text
src/
  main.rs              CLI dispatch and daemon bootstrap
  cli/                 arguments, IPC, headless command execution
                       (build.rs generates a project shell; extension.rs
                       compiles the crates config.lua enables)
  shell/
    state.rs           domain coordination and event dispatch
    windows.rs         surface registries, output identity and idempotent detachment
    context.rs         host-to-service snapshot adapter
    input.rs           owned window cursors, global cursor and press identity
    desktop.rs         selection/fade lifecycle, context menu, chrome runtimes, wallpapers
    wallpaper.rs       serialized bake jobs, draft/apply validation and publication
    catalog.rs         discovered widget definitions and library change stamps
    animation.rs       shared motion clock and widget-list lifetime
    notifications.rs   notification service queue and ID allocation
    overlay.rs         shared fullscreen surfaces, source runtimes, measured input masks
    events.rs          host messages
    jobs.rs            persistence and theme-regeneration job ownership
    apps.rs            framework app instances and their declared layer roots
    apps/popup.rs      app-owned modal child surfaces
    screens/           bar, wallpaper/menu, popup, notification, settings hosts
      top/             bar host
        scheduling.rs  due placement selection and execution with an injected clock
        runtime.rs     placement revision/render/ingest adapter with explicit borrows
        render.rs      bar and slot composition over cached placement views
        input.rs       bar input identity, slot hit-testing and press matching
        edit.rs        live geometry/slot edits and sparse persistence snapshots
        animation.rs   list-animation synchronization context
      notification.rs  delivery/output/expiry/action policy and scoped scene data
      popup/
        surface.rs     popup geometry, list synchronization and registration
      setting/         Settings host, explicit page snapshots and shared controls
        animation.rs   cached Lua Animation page and validated speed actions
        theme_page.rs  cached Lua Theme page, native icon loading and validated actions
        panel_bar.rs   cached Lua bar-level Panel controls and typed bar/style actions
        components.rs  cached Lua Components editor; native literal parsing and validation
        desktop_page.rs cached Lua Desktop page with host-filled widget preview slots
        context_menu.rs cached Lua context-menu source page
        wallpaper.rs   cached Lua wallpaper editor with a native map-canvas slot
        panel.rs       Panel composition and immutable bar/catalog/output context
        placement.rs   cached Lua placement editor, catalog snapshots and typed action validation
        sources.rs     project/bundled Settings page resolution and hot-reload stamps
  ui/
    scene.rs           measured keyed lists, native layout, host slot resolution, inert full-canvas exits
    placement.rs       reusable parent-local anchor geometry
    layout_props.rs    common anchor/margin/press modifiers for all components
    image_value.rs     opaque Lua image tokens decoded to owned native handles
    assets.rs          bounded, revisioned full-resolution image-handle cache
    node.rs            owned description IR
    decode.rs          Lua description decoding
    dsl.rs             shared constructor/setter bindings
    build.rs           iced realization and legacy popup size estimates
    icons.rs           generated icon lookup and text placeholders
    anim.rs            motion types/runtime wrapper
    listview.rs        keyed list transitions and ghosts
    motion.rs          draw translation and matching input coordinates
    style.rs           common editor/input styling
    widgets/           measured anchoring, event shells, controls, editor canvases
  lua/
    runtime.rs         bounded app host and cached owned views
    sandbox.rs         explicit app/widget VM profiles
    entry.rs           common self-bound method invocation
    error.rs           shared Lua diagnostics and deduplication
    library.rs         profile-scoped library validation and last-good caching
    props.rs           property merging and publication adapters
    bridge.rs          palette snapshot publication
    composable.rs      source-backed chrome instances and last-good reloads
    widgets.rs         placement VMs, entry publication and per-window render caches
    metadata.rs        widget author defaults and property-schema decoding
    notifications.rs   notification app entries, metadata and owned action decoding
    shell.rs           Lua-requested host shell commands (add top, open settings)
    transitions.rs     transition decoding
    value.rs           scalar coercion and diagnostics
    demo.rs            development host
    seed_tests.rs      bundled widget contracts using canonical Lua/UI APIs
    test_support.rs    explicit bundled-library and service fixtures (tests only)
  config/
    mod.rs             public configuration API
    schema.rs          persisted types and pure transformations
    discovery.rs       widget/component discovery and change stamps
    migrate.rs         legacy conversions and component-library migration
    util.rs            path expansion and image decoding
    io.rs              loading, polling, persistence and parse diagnostics
    paths.rs           XDG-aware path resolution
    seed.rs            bundled assets and one-time initialization policy
  theme/
    mod.rs             theme API, resolved palette and daemon hooks
    schema.rs          serialized theme-file schema
    store.rs           builtin/user theme sources and resolution
    cache.rs           active-palette cache and polling
    style.rs           iced tokens and status-aware paint factories
    lua.rs             palette-to-Lua representation
  theme_gen/
    mod.rs             generation API
    variant.rs         Material-You variant names
    views.rs           wallpaper rasterization and combination
    scheme.rs          scheme construction, JSON and stored-theme reads
    templates.rs       template installation, substitution and hooks
  icons.rs              shared size/theme-aware icon paths, inheritance and cache
  icons/tests.rs        isolated freedesktop theme fixtures
  notify/
    server.rs          notification transport and ingestion
    images.rs          pixbuf/file/SVG ingestion
  services/            system and native Wayland snapshots
    state.rs           owned sampler and native listener caches
  shared/geometry.rs   output geometry independent of UI/window ownership
  wallpaper/
    mod.rs             output snapshots, derived tile cache and transactional commit
    raster.rs          original-pixel sampling and premultiplied linear compositing
    draft.rs           isolated editable recipe and preview-buffer ownership
```

The former `app`, `components`, `composables`, and `colorgen` import facades
have been removed. Callers use `shell`, `ui/widgets`, and `theme_gen` directly.

## Data flow

The shell owns lifecycle, input routing, output identity, and persistence.
It assembles read-only `ServiceCtx` snapshots at `shell/context.rs`.
Services publish data; protocol listeners do not depend on window geometry
or UI rendering. Shared output geometry lives in `shared/geometry.rs`.

Lua constructs description tables through `ui/dsl.rs`. The decoder produces
owned `WidgetNode` values, and `ui/build.rs` realizes them into iced Elements.
Button actions are strings routed by a host-provided callback. Lua never
retains an iced Element or a Rust state reference.

App hosts, source-backed chrome, and widget placements share bindings,
property helpers, scalar diagnostics, transition parsing, and palette
publication. Method invocation uses `lua/entry.rs`; argument adapters remain distinct:
widgets read `self.props`, composables receive `view(props)`, and the demo
retains its window-ID entry convention. This preserves existing scripts.

VM profiles are explicit. The widget profile retains native `os.execute`
and `io.popen` with the existing blocked functions. Both profiles now use
the shared 200,000-instruction entry budget and 64 MiB VM memory limit.
Only outer entries reset fuel; nested components share the outer budget.
Budget failures are recoverable, and library validation uses the same limits.
VMs remain separate per placement/chrome instance.

Passive daemon, bar, background and notification surfaces request no keyboard
focus. Notification surfaces are warmed per output and remain mapped with an
empty input region when idle; arrivals reuse the existing surface/renderer,
avoiding creation-time fullscreen/XWayland focus handoffs. Card masks are
installed after native surface creation and refreshed as the stack changes.
Mask commits only fire when the card rects actually move, and the passive
keyboard policy is asserted once per native surface — never on every update.

## Assets and migrations

Bundled assets remain in `scripts/`, `themes/`, and `templates/`. Installed
configuration paths and Lua `src` values do not change when Rust modules
move. Existing Lua libraries are user-owned. Initialization markers remain
outside the widget/component directories; deleted files stay deleted.
Legacy config conversion and backups retain their existing semantics.

Wallpaper Settings use an editor-owned draft. Preview events redraw only the
Settings window; Apply snapshots source buffers and output geometry, bakes
lossless native-resolution tiles on a serialized worker, and publishes only
the latest valid result. Recipe persistence is atomic and cache-manifest
publication is rolled back on persistence failure. Originals remain editable
and untouched. Desktop surfaces use one applied tile per output; original
decoded buffers are held by the editor/bake job rather than the desktop.
The recipe retains both Free Form placements and name-keyed Per Monitor
assignments. Per Monitor placement resolves fit/expand/stretch/center in oriented
native pixels, then converts to desktop coordinates; each output is composited
independently onto opaque black. Preview loading only includes connected active
sources, and the map clips each preview to its output. Version 3 manifests encode
this compositing policy; legacy Free Form manifests remain restorable. Cached
output geometry is sorted by name for deterministic headless theme sampling.
Preview sources carry file stamps shared with the generic UI asset cache. Reloads
replace only changed sources; Apply jobs retain immutable accepted source buffers.
Cache restoration validates the complete manifest before decoding in batches of
at most four outputs, joins every worker, and publishes only complete batches.
Placement ingestion reports content changes without cloning before/after trees;
unchanged trees and declared lists keep their state while Lua entries still run.
Popup measurements use a bounded content/width/text-size/output-height cache
across windows, with image-dependent trees excluded and renderer confirmation
retained even when a measurement does not require a native resize.
Dynamic-theme generation uses applied output composites. Generic UI image
handles are cached independently without changing their pixel dimensions.
Wallpaper publication carries an explicit origin: editor Apply can request
dynamic-theme generation, while startup restoration and output rebuilds only
publish tiles. Palette selection and variant changes do not schedule generation;
an unchanged explicit Apply still requests it, so pending variant choices apply.
Palette-file refresh has its own event, preserving pending saves, placement
IDs, VM self state and open popups. Real config reloads only rebuild bar
instances when persisted bar recipes differ (runtime IDs are ignored).
Dynamic-theme sampling borrows resident applied RGBA buffers and resizes a
virtual stitched image directly to 128×128, eliminating full-resolution
stitch/PNG/decode copies. Completion schedules reclamation after host-side
result processing as well as trimming worker temporaries.
Native `Closed` events perform idempotent app detachment without requesting
another close for the dead native ID. Image reclamation is deferred until
result-message and renderer/UI owners can drop; cache entries still referenced
by active elements remain intact. Closes of windows that owned decoded images
repaint every surface so live renderers run their insert-gated image-cache trim
(per-renderer caches die with their windows; the trim frees host entries and
atlas slots for reuse, it does not shrink GPU pages). Wallpaper publication
compares actual RGBA
pixels and reuses previous handles for identical tiles, keeping texture IDs
stable across repeated Apply operations.

## Migration status and remaining work

The structural foundation is in place: shared UI code no longer belongs to
the bar layer, shell modules have a distinct namespace, CLI execution is
outside bootstrap, and palette/props/geometry/input-style duplicates have
shared owners. The historical DSL oracle has been removed; representative
builder descriptions assert expected owned IR directly in `ui/dsl.rs`.

Completed in the continuation:

- Config, theme, notification ingestion and theme-generation jobs have
  separate implementations, with the existing tests moved alongside them.
- Placement tick scheduling and narrow list-animation synchronization
  have separate bar adapters. Theme/Animation Settings pages take snapshots;
  property-row presentation and patch messages live in `setting/editors.rs`.
- Persistence and regeneration jobs have domain-owned state. Their timer
  generation rules retain superseded-timer and reload invalidation behavior.
- Pointer/press tracking and service samplers/listener caches have separate
  owners. Snapshot-only Panel/Wallpaper/Context Menu contexts expose just
  their required inputs, rather than the complete shell state.
- Composable and widget property controls share one builder, preserving
  inferred types, schema bounds/choices, inherited captions and sparse resets.
- Library validation, last-good selection, Lua diagnostics, call binding,
  and execution budgets use shared implementations with regression coverage.
- Compatibility import facades have been removed after migrating callers.
- Panel and placement editor composition, including slider/image reset rows,
  have moved out of the Settings lifecycle host.
- Placement VMs and their caches have a shared owner in `lua/widgets.rs`;
  VM construction, library installation, app validation, entry publication
  and script-revision invalidation live alongside it. Render/action/press
  entries publish fresh services, `bar.output` and `self.props` through an
  explicit `EntryContext`; stale host messages are ignored. Regression
  coverage checks independent placement state, failed-load cleanup,
  shared-path revision invalidation and fresh entry publication.
- Desktop selection/fade lifecycle, context-menu state, composable runtimes
  and pre-decoded wallpaper handles are grouped in `shell/desktop.rs`,
  shared by background and Settings hosts.
- Discovered widget definitions, directory/library change stamps and the
  retired-file warning have a catalog owner in `shell/catalog.rs`.
- The shared motion clock and widget-list lifetime moved to
  `shell/animation.rs`; notification queue, renderer, sizes,
  scroll, exit images and trees moved to `shell/notifications.rs`, with the
  stack view taking a read-only snapshot context.
- Bar scheduling selects due placements against injected last-run, popup and
  clock inputs; execution, revision sync and render ingestion run through
  `top/runtime.rs` with explicit placement/catalog/animation/service borrows.
  Bar composition moved to `top/render.rs`, and input identity, slot
  hit-testing and press matching to `top/input.rs`.
- Widget author defaults and property-schema decoding moved to
  `lua/metadata.rs`. Popup/background/notification hosts import shared Lua
  and UI machinery from its canonical modules instead of the bar host.
- Builder and icon regression tests moved to `ui/dsl.rs` and `ui/icons.rs`
  alongside their implementations.
- Bar geometry, visual settings, slot layout and placement-property edits
  now use `top/edit.rs::EditContext`, borrowing only the bar registry,
  output identities/geometry, catalog definitions and saved bar entries.
  The shell coordinates rendering and coalesced-save commands. Regression
  coverage checks sparse property persistence, stable bar entry indices,
  sentinel layout behavior and stale edits.
- Widget contracts, self binding, sandbox restrictions/native shell,
  author metadata, property publication and transition invocation tests
  have moved from the bar host to their Lua owners. Tree-decoding and
  rich-text realization tests moved to `ui/decode.rs` and `ui/icons.rs`.
  Transition entry invocation now lives in `lua/transitions.rs`.
- Surface identities, bar/popup/Settings/background registries, output
  geometry and notification-window bindings now belong to `WindowState` in
  `shell/windows.rs`. The old flat registry fields and per-screen output
  removal helpers have been removed after migrating every caller.
- Surface detachment coordinates input, per-window placement caches and
  bar/popup animation scope cleanup. Compositor bar closes dismiss child
  popups without deleting saved bar entries; output removal preserves other
  outputs and Settings windows. Notification output teardown releases ghost
  images and motion immediately. Tests cover repeated detachment, replacement
  background protection, child/cache cleanup and output isolation.
- Primitive realization, spinner composition and tinted-node rendering
  tests moved to `ui/build.rs`, using shared constructors directly.
- Notification arrivals, retirement, expiry and action guards use a
  queue/motion context; render entries use a separate renderer/cache/list
  context with read-only service inputs. `lua/notifications.rs` owns VM
  revision tracking, payload publication, self-bound views and owned action
  decoding. Library installation occurs once per notification VM.
- Notification output selection and native creation use output/window/size
  snapshots. Popup creation and refresh use window and list-animation
  contexts; popup/action entries share `lua/widgets.rs::EntryContext` with
  widget views and presses. Bar press dispatch borrows only window identity
  and input state.
- Notification input-mask fingerprints are per output, including native
  surface invalidation and output teardown. A two-output regression guards
  identical card layouts from suppressing each other's mask updates.
- The remaining shared constructor/style/delegate tests moved into `ui/`,
  script/seed contracts into `lua/`, system publication into `services/`,
  and path resolution into `config/`. Popup-specific tests live with the
  popup host and list-animation tests with the bar animation adapter.
  `top/mod.rs` retains bar-specific tests and no test-only Lua/UI re-exports.

Further consolidation:

Shell coordinators still assemble contexts and batch cross-domain commands.
Bar bootstrap/config persistence and desktop/menu event coordination can
adopt the same context pattern. File-size targets remain advisory; ownership
and dependency boundaries determine extraction scope.

## Verification

Notification records and popup visibility have separate lifetimes. Expiry hides
the popup and closes its D-Bus lifetime once, while the metadata center retains
the record until explicit dismissal, invocation, or peer close. IDs are shared
between transport/internal notifications; replacement generations guard stale
completions. `notification.dismiss(id)` / `dismissAll()` construct returned
commands, handled identically in scene, widget, and popup actions.

Explicit listview transitions can capture bounded completion callbacks through
opaque registry tokens with weak VM ownership. Native motion sets collect
completions, and the host dispatches them after sweeping, outside layout/draw.
App epochs, item generations, interrupted motions, and surface teardown prevent
stale callbacks; registry tokens and captured item references are released when
their owners/ghosts are gone. Scene displacement callbacks follow measured
bounds, without a second index-based completion.

`config/format.rs` defines the human-facing TOML representation separately from
resolved Rust layout fields. Bar slots group alignment and widget placements;
monitor wallpapers are keyed by output; free-form images use `[x, y]` positions;
bar margin/radius accept uniform numbers or named tables. `format_migration.rs`
converts old flat keys once on file load, backed up as `config.toml.pre-structured`.
Settings saves compact slots/geometry and omits defaults; widget inheritance is
preserved instead of materializing Lua defaults into placement overrides.

Composables use one optional `[composable]` table of source strings. Default
filenames are omitted on save; sizing and appearance live in Lua `app.defaults`.
Old source tables migrate at the file-load boundary, with the original config
preserved as `config.toml.pre-composable-sources`; TOML style overrides are retired.

Lua source paths share `lua::composable::source_path`: composable sources,
notification/overlay `source`, and `riced.component()` resolve relative names
under `~/.config/riced/components/`; absolute paths pass through. There is no
config-root fallback. The notification scene seed lives at
`scripts/components/notifications.lua` and installs into that same directory.

The iced Lua component schema lives in `ui/components.rs`: one declaration
generates named owned Rust descriptions, the `WidgetNode` variants, conversions,
and constructor/setter metadata. `ui/dsl.rs` installs the sole `iced` namespace;
`ui/encode.rs` exports chainable descriptions into a VM.
`ui/action.rs` carries owned keys and boolean/number/string values. Native
callbacks use an owned routing sender, never a borrowed Lua VM. Placement and
demo hosts dispatch owned event tables to `actions(events)` and invalidate their
views. Native input/layout tests cover controls, disabled behavior, exports,
and cross-VM ownership in addition to the seed contracts.

The placement input shell delivers unconsumed pointer gestures and keyboard
events after native controls. Desktop left holds become `drag` events; `self.drag()`
starts capture by placement identity. Held-drag previews are pointer-transparent
and never request keyboard focus; the initiating background surface retains the
implicit pointer grab. Only its matching button release, tagged with the current
gesture identity and release-time pointer position, sends
`drop` back to the initiating VM, where `self.drop()` commits the position. Declined
or failed drops cancel; stale sessions and preview-surface releases are ignored.
Settings uses a separate, input-taking two-click placement interaction.
Both host methods are scoped to their matching event in `actions`. Hold duration
is configured globally in `[interaction].hold` (preset or custom milliseconds).
Repositioning commits only output/x/y and preserves the placement's appearance.
The input shell retains the press origin separately from current pointer coords.
Capture previews and drops share `cursor - grab_offset`, including negative
origins at output edges. Motion from the initiating Wayland surface is translated
through logical output geometry until its implicit pointer grab ends. Settings
renders isolated per-window/widget/output previews, measured at native layout
size and scaled for display. Preview hit testing inversely maps only the visible
widget rectangle, and press-time revision tokens reject stale picks. Drag ghosts
retain the accepted source tree; closing Settings drops its preview VMs/caches.

Run `just check` for formatting, the complete test suite, clippy, and diff
whitespace checks in the Nix development environment. Test behavior at
boundaries: Lua contracts, placement identity, fixed/automatic layout,
shifted input, notification masks, native service snapshots, reload fallback,
and one-time initialization. RSS probes execute in an isolated test process
so parallel Lua/renderer allocations cannot contaminate memory measurements.
Do not replace these with tests of file names.

Manual release memory probes must run separately to keep allocator warm-up
and peak RSS independent:

```bash
nix develop --command cargo test --release palette_sampling_memory_probe -- --ignored --nocapture --test-threads=1
RICED_SAMPLE_REFERENCE=1 nix develop --command cargo test --release palette_sampling_memory_probe -- --ignored --nocapture --test-threads=1
nix develop --command cargo test --release settings_decode_memory_probe -- --ignored --nocapture --test-threads=1
RICED_DECODE_REFERENCE=1 nix develop --command cargo test --release settings_decode_memory_probe -- --ignored --nocapture --test-threads=1
```

The palette probe uses a procedural 7680×2160 source, isolating scratch memory.
The decode probe loads and releases a 3840×2160 PNG six times on a persistent
blocking pool. These measure CPU allocation behavior; live compositor/renderer
memory requires a separate daemon session. Ownership regressions cover native,
explicit, and toggle closes, plus late/stale preview results.
