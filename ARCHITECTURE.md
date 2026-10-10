# Architecture

## Module ownership

```text
src/
  main.rs              CLI dispatch and daemon bootstrap
  cli/                 arguments, IPC, headless command execution
  shell/
    state.rs           window/state ownership and event dispatch
    context.rs         host-to-service snapshot adapter
    input.rs           owned window cursors, global cursor and press identity
    events.rs          host messages
    jobs.rs            persistence and theme-regeneration job ownership
    screens/           bar, wallpaper/menu, popup, notification, settings hosts
      top/             bar host, placement scheduling, list-animation context
      setting/         Settings host, explicit page snapshots and shared controls
        context_menu.rs source-backed chrome property page
        wallpaper.rs   wallpaper map and image-property composition
        panel.rs       immutable bar/catalog/output context
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
shared owners. The old DSL implementation is retained **only in tests** as
a temporary compatibility oracle for the extracted binding implementation.

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

Remaining work:

1. Move the remaining Panel composition and placement/image helper rows out
   of the Settings host. Wallpaper and Context Menu composition and common
   schema controls have already moved; finish the remaining bar render
   adapters and placement-runtime management extraction.
2. Continue replacing the flat window/widget/notification fields in `Plots`
   and broad handler borrows with domain-owned state and narrow contexts.
3. Relocate remaining historical DSL tests and remove their test-only oracle.
   Keep file-size targets advisory until these extractions land.

## Verification

Run `just check` for formatting, the complete test suite, clippy, and diff
whitespace checks in the Nix development environment. Test behavior at
boundaries: Lua contracts, placement identity, fixed/automatic layout,
shifted input, notification masks, native service snapshots, reload fallback,
and one-time initialization. RSS probes execute in an isolated test process
so parallel Lua/renderer allocations cannot contaminate memory measurements.
Do not replace these with tests of file names.
