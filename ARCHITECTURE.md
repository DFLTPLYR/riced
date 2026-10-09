# Architecture

## Module ownership

```text
src/
  main.rs              CLI dispatch and daemon bootstrap
  cli/                 arguments, IPC, headless command execution
  shell/
    state.rs           window/state ownership and event dispatch
    context.rs         host-to-service snapshot adapter
    events.rs          host messages
    screens/           bar, wallpaper/menu, popup, notification, settings hosts
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
    props.rs           property merging and publication adapters
    bridge.rs          palette snapshot publication
    composable.rs      source-backed chrome instances and last-good reloads
    transitions.rs     transition decoding
    value.rs           scalar coercion and diagnostics
    demo.rs            development host
  config/
    mod.rs             schema, discovery, migration, public facade
    io.rs              loading, polling, persistence and parse diagnostics
    paths.rs           XDG-aware path resolution
    seed.rs            bundled assets and one-time initialization policy
  theme/
    mod.rs             theme resolution, palette cache and style facade
    schema.rs          serialized theme-file schema
  theme_gen/           wallpaper-derived themes and template generation
  notify/
    server.rs          notification transport and ingestion
    images.rs          pixbuf decoding
  services/            system and native Wayland snapshots
  shared/geometry.rs   output geometry independent of UI/window ownership
```

`app`, `components`, `composables`, and `colorgen` are temporary **import
facades**, not second implementations. New code should use `shell`,
`ui/widgets`, and `theme_gen` directly. Existing callers continue to compile
while their domain modules are extracted.

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
publication. Their scheduling and view-call adapters remain distinct:
widgets read `self.props`, composables receive `view(props)`, and the demo
retains its window-ID entry convention. This preserves existing scripts.

VM profiles are explicit. The widget profile retains native `os.execute`
and `io.popen` with the existing blocked functions. App-host instruction and
memory limits retain their existing behavior. Changing limits or moving to
one VM is a separate behavioral migration, not a file move.

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

The full multi-phase refactor still includes:

1. Split the remaining config schema/discovery/migration, theme
   store/cache/styles, image/icon ingestion, and theme-generation jobs.
2. Extract placement scheduling and bar animation adapters from the bar
   host, then split Settings pages and property editors.
3. Replace the flat `Plots` state and broad handler borrows with domain
   ownership and narrow state/context interfaces.
4. Unify library invalidation and Lua error reporting, then migrate call
   adapters and execution budgets with dedicated compatibility tests.
5. Remove compatibility facades and the test oracle after callers/tests
   have moved. Keep file-size targets advisory until these extractions land.

## Verification

Run `just check` for formatting, the complete test suite, clippy, and diff
whitespace checks in the Nix development environment. Test behavior at
boundaries: Lua contracts, placement identity, fixed/automatic layout,
shifted input, notification masks, native service snapshots, reload fallback,
and one-time initialization. Do not replace these with tests of file names.
