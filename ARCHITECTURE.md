# Architecture

## Module ownership

```text
src/
  main.rs              CLI dispatch and daemon bootstrap
  cli/                 arguments, IPC, headless command execution
  shell/
    state.rs           domain coordination and event dispatch
    windows.rs         surface registries, output identity and idempotent detachment
    context.rs         host-to-service snapshot adapter
    input.rs           owned window cursors, global cursor and press identity
    desktop.rs         selection/fade lifecycle, context menu, chrome runtimes, wallpapers
    catalog.rs         discovered widget definitions and library change stamps
    animation.rs       shared motion clock and widget-list lifetime
    notifications.rs   notification queue, renderer and per-output presentation state
    events.rs          host messages
    jobs.rs            persistence and theme-regeneration job ownership
    screens/           bar, wallpaper/menu, popup, notification, settings hosts
      top/             bar host
        scheduling.rs  due placement selection and execution with an injected clock
        runtime.rs     placement revision/render/ingest adapter with explicit borrows
        render.rs      bar and slot composition over cached placement views
        input.rs       bar input identity, slot hit-testing and press matching
        edit.rs        live geometry/slot edits and sparse persistence snapshots
        animation.rs   list-animation synchronization context
      setting/         Settings host, explicit page snapshots and shared controls
        context_menu.rs source-backed chrome property page
        wallpaper.rs   wallpaper map and image-property composition
        panel.rs       Panel composition and immutable bar/catalog/output context
        placement.rs   placement properties and catalog-based editor
        editors.rs     schema-driven scalar/choice controls and reset rows
  ui/
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
    transitions.rs     transition decoding
    value.rs           scalar coercion and diagnostics
    demo.rs            development host
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
  notify/
    server.rs          notification transport and ingestion
    images.rs          pixbuf/file/SVG ingestion
    icon_theme.rs      freedesktop icon lookup and bounded cache
  services/            system and native Wayland snapshots
    state.rs           owned sampler and native listener caches
  shared/geometry.rs   output geometry independent of UI/window ownership
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

Remaining work:

1. Continue narrowing cross-domain window creation, popup/input dispatch
   and notification arrival/render/action handlers into explicit contexts.
2. Relocate the remaining shared Lua/UI tests still living in the bar host
   to their canonical modules, and drop the test-only re-exports left in
   the bar host. Keep file-size targets advisory until these land.

## Verification

Run `just check` for formatting, the complete test suite, clippy, and diff
whitespace checks in the Nix development environment. Test behavior at
boundaries: Lua contracts, placement identity, fixed/automatic layout,
shifted input, notification masks, native service snapshots, reload fallback,
and one-time initialization. RSS probes execute in an isolated test process
so parallel Lua/renderer allocations cannot contaminate memory measurements.
Do not replace these with tests of file names.
