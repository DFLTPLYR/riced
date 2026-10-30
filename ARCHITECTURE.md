# Architecture

Riced is a local Wayland desktop daemon. Lua describes UI and interaction state;
Rust owns native windows, rendering, input routing, services, validation, and
persistence. The expected load is one desktop session with several outputs and
widgets, not a multi-user server. Run only one daemon per session.

For setup and project paths, start with [README.md](README.md). API contracts
live in the [framework](docs/framework.md), [Lua](docs/lua-api.md),
[persistence](docs/persistence.md), and [extension](docs/extensions.md) guides.

## Module ownership

| Path | Owns |
| --- | --- |
| `src/main.rs`, `src/lib.rs` | Binary entry, CLI dispatch, daemon bootstrap, project registry |
| `src/cli/` | Arguments, queued IPC, headless commands, generated project builds |
| `src/framework/project.rs` | App/extension configuration, entry resolution, source manifests |
| `src/framework/tasks.rs` | Managed task execution and owned deliveries |
| `src/framework/storage.rs` | Bounded document codecs, serialized atomic file operations |
| `src/shell/state.rs` | Host state, subscriptions, event coordination |
| `src/shell/daemon.rs` | Lua title/theme/style/redraw callback adapters |
| `src/shell/apps.rs` | App instances, shared daemon runtime, surface reconciliation |
| `src/shell/apps/popup.rs` | App-owned native popup children |
| `src/shell/windows.rs` | Surface registries, output identity, idempotent detachment |
| `src/shell/input.rs` | Window cursors, global cursor, press identity |
| `src/shell/desktop.rs`, `desktop_widgets.rs` | Legacy desktop hosts, widget placement/drag state |
| `src/shell/wallpaper.rs`, `jobs.rs` | Bake/apply coordination, persistence and theme job ownership |
| `src/shell/notifications.rs`, `screens/notification.rs` | History, output assignment, expiry, validated actions |
| `src/shell/catalog.rs`, `animation.rs` | Widget discovery, library changes, motion clocks |
| `src/shell/overlay.rs` | Legacy scene hosts and measured input masks |
| `src/shell/screens/top/` | Legacy native bar scheduling, rendering, editing, input |
| `src/shell/screens/popup/` | Legacy widget-popup placement, measurement, lifecycle |
| `src/shell/screens/setting/` | Settings snapshots, validated actions, wallpaper draft/map host |
| `src/lua/runtime.rs`, `daemon.lua`, `root.rs` | App loading, daemon builder, startup settings, cached views |
| `src/lua/managed.rs`, `files.rs` | VM-owned callbacks, managed methods, document handles/watchers |
| `src/lua/embedded.rs` | Embedded widgets/plugins and action/popup routing |
| `src/lua/widgets.rs`, `composable.rs` | Placement and source-backed chrome runtimes |
| `src/lua/sandbox.rs`, `entry.rs`, `library.rs` | VM profiles, bounded invocation, validated shared libraries |
| `src/ui/components.rs`, `dsl.rs`, `decode.rs`, `encode.rs` | Component schemas, Lua bindings, owned descriptions |
| `src/ui/build.rs`, `scene.rs`, `widgets/` | iced realization, measured scenes, input/layout controls |
| `src/ui/animations.rs`, `listview.rs`, `motion.rs` | Native property/list animation, exit ghosts, translated input |
| `src/ui/assets.rs`, `shader.rs` | Revisioned image cache and GPU fragment rendering |
| `src/config/` | Preference codecs, discovery, seeding, installation, layout migration |
| `src/services/` | Cached system, theme, notification, tray, Wayland snapshots |
| `src/notify/`, `src/tray/` | D-Bus transports, image ingestion, tray menus |
| `src/theme/`, `src/theme_gen/` | Applied palette, source polling, wallpaper sampling, templates |
| `src/wallpaper/` | Source/draft ownership, native-pixel rasterization, cache publication |
| `src/shared/` | Shared geometry, file, and pixel utilities |
| `crates/` | Extension SDK and bundled Rust service crates |
| `scripts/` | Bundled Lua sources and shaders installed into user projects |

The bundled shell uses Lua layers. Legacy native bar/background/overlay hosts
remain for projects that still configure them; the bundled shell does not use
all of these hosts.

## Startup and source loading

1. Resolve the project, move legacy paths without overwriting conflicting files,
   and initialize a missing default frontend.
2. Load `shell.lua` (`root.lua` fallback) before connecting to Wayland. Validate
   startup settings and read the namespace callback.
3. For a daemon entry, retain that Lua VM for all outputs, even when none are
   connected. Otherwise use plain per-output app instances.
4. Reconcile Lua declarations against native surfaces as outputs/state change.
   Native closes detach ownership once.

Source manifests track app entries, components, layers, and composables.
Replacement apps mount and validate their first view before taking over;
invalid candidates retain the working runtime. Successful reloads reset app
state and retire old callbacks/jobs. Data files outside the source manifest do
not reload app code.

Installation preserves user files. Markers and locks live under `data/state/`;
ordinary startup does not reseed deliberately deleted libraries. Explicit repair
restores missing frontend defaults, not source updates. See
[editing and upgrades](README.md#editing-and-upgrades).

## UI and event flow

Lua constructs descriptions, not live iced elements. Rust decodes owned
`WidgetNode` values, caches them, then realizes iced elements. Lua never receives
a mutable shell-state reference.

Controls emit owned action keys and optional values with native surface identity.
Hosts route them to the owning widget, app `actions`, or daemon `update`.
Daemon layer actions include source-output geometry. Declaring/omitting a surface
controls visibility; stable IDs preserve native identity.

`components/settings/window.lua` declares Settings with a native plugin leaf.
Lua owns page layout/navigation; native adapters supply snapshots and validate
edits before changing preferences or applying wallpaper drafts.

Source-backed composables resolve relative names under `composables/`, with
`components/` as a legacy fallback. Framework modules also search app/project
paths and their component directories. Selecting a project never changes the
process working directory.

Animations run on native clocks, not per-frame Lua. Descriptions capture
VM-owned callback tokens; completions run during updates, never layout/draw.
Epochs, item generations, cancelled motion, and teardown reject stale callbacks.
Exit ghosts retain their content until their transition ends.

## Services and background work

`ServiceCtx` publishes cached snapshots before ordinary app entries. Protocol
listeners own service data, not UI geometry. Subscriptions coalesce updates;
views do not start extension workers or perform service I/O.

Managed processes, timers, document I/O, and extension commands return owned
deliveries. The host checks the generation/token before accepting callbacks or
replacement snapshots. Cancellation/reload suppress late delivery, not side
effects already performed by a worker.

Compiled-in extensions run trusted Rust with bounded worker concurrency. Process
services use a bounded stdio protocol, start on the first command, and restart on
the next request after failure. Neither backend is an OS sandbox. See
[extension limits](docs/extensions.md).

## Persistence and image ownership

Settings writes `data/config.lua`. The bundled bar editor separately reads and
writes `data/bars/*.lua`; `riced.file` gives app-owned documents asynchronous
reload/save/watch with conflict detection. Synchronous helpers do not provide
the managed-document guarantees.

Wallpaper Settings keeps a draft separate from the applied recipe. Apply retains
accepted pixels and output geometry, bakes lossless native-resolution tiles,
then publishes only the latest valid result. Changed/deleted/unreadable config
blocks publication; persistence failure restores the previous manifest. The
final config check is optimistic, not a cross-process lock.

Free Form and Per Monitor recipes remain separate. Per Monitor uses name-keyed
assignments, native-pixel fit/expand/stretch/center, and opaque black outside
images. Output geometry is sorted for deterministic headless theme sampling.
Cache restoration validates complete manifests and decodes at most four outputs
concurrently.

Desktop surfaces retain applied tiles, not editor originals. Source stamps reuse
unchanged buffers; identical applied pixels reuse handles and texture IDs.
Large editor sources use anonymous read-only pixel mappings, released by their
last owner even across threads. Original files are neither mapped nor changed.

Dynamic-theme generation samples resident applied pixels into a bounded 128×128
image without a full-resolution stitched copy. Explicit Apply can request it;
startup restoration, output changes, theme selection, and variant edits do not.

Notification visibility and history have separate lifetimes. Expiry emits one
close signal but leaves the center record. History budgets do not bound all
image references or process memory: snapshots, exit ghosts, and renderer caches
can retain pixels after queue removal.

Removal/close schedules deferred reclamation: expire discarded Lua registry
keys, collect framework/widget VMs, clear unreferenced images, and trim free
allocator pages where supported. Live owners remain intact; renderer/driver
pools may keep their capacity. Cleanup is not forced every frame. Native close
confirmation never requests another native close.

## Validation

```sh
just check
nix develop --command cargo test --workspace --locked
```

Tests cover Lua/action contracts, budgets, reload fallback, surface identity,
notification policy/images, file conflicts, extension protocol/cancellation,
initialization, and measured layout/input. Headless tests do not replace live
multi-monitor, compositor focus/grab, or GPU checks.

Manual release memory comparisons run separately to keep allocator warm-up and
peak RSS (resident process memory) independent:

```sh
nix develop --command cargo test --release palette_sampling_memory_probe -- --ignored --nocapture --test-threads=1
RICED_SAMPLE_REFERENCE=1 nix develop --command cargo test --release palette_sampling_memory_probe -- --ignored --nocapture --test-threads=1
nix develop --command cargo test --release settings_decode_memory_probe -- --ignored --nocapture --test-threads=1
RICED_DECODE_REFERENCE=1 nix develop --command cargo test --release settings_decode_memory_probe -- --ignored --nocapture --test-threads=1
```

The palette probe uses a procedural 7680×2160 source; the decode probe loads and
releases a 3840×2160 PNG six times. They measure CPU allocations, not compositor
or GPU memory. RSS regressions use isolated processes to avoid parallel-test
contamination.
