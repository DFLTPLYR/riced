# Rust extensions (experimental SDK v1)

Extensions are ordinary Cargo library or executable crates, usually under
`extensions/<name>/`. `crates/riced-extension-sdk` is version `0.1.0`, with
source contract `SDK_VERSION = 1`. It depends on neither Lua nor iced. This is
not a stable Rust ABI: compiled-in extensions use path dependencies and require
rebuilding and restarting riced. Supervised executable services are also
supported through versioned stdio, without linking their code into the shell.
No dynamic `.so` loading or runtime directory scanning is implemented.

## Registration and ownership

Implement `Extension` to return a `Manifest` and service implementations.
Each `Service` declares its name and supported commands, supplies an initial
snapshot, and executes commands over owned JSON data. Each extension crate
exposes one entry point used by generated shells:

```rust
pub fn register(
    registry: &mut riced_extension_sdk::Registry,
    reserved: &[&str],
) -> Result<(), riced_extension_sdk::Error> {
    registry.register(&Palette, reserved)
}
```

`reserved` is `riced::builtin_service_topics()`; it keeps extension services
from shadowing built-in topics. The default `riced` binary registers the
`palette` and `theme_gen` extensions for the bundled reference shell. Project
shells are built from the extensions enabled in `config.lua`; no framework
source edits are needed. Extensions are session-wide, not per project/output,
and do not restart when an app reloads.

## Enabling and building

List extensions in `config.lua`:

```lua
return {
    extensions = {
        { name = "palette" },                    -- extensions/palette
        { name = "custom", path = "vendor/custom" },
    },
}
```

`path` is resolved from the project root and defaults to `extensions/<name>`
(absolute paths are accepted). The path must contain a Cargo library crate that
depends on `riced-extension-sdk` and exposes `register`.

```sh
riced build -p .                 # debug shell
riced build -p . --release       # optimized shell
riced build -p . --locked        # require the recorded lockfile
```

`riced build` writes a small Cargo package under `.riced/` (`Cargo.toml`,
`src/main.rs`, and `Cargo.lock`) and builds it into
`target/riced-shell/<profile>/riced-shell-<project>`. The generated `main`
registers each enabled extension and calls `riced::run_project`, so the
resulting binary defaults to that project and still accepts `-p`/`-c`.

- Commit `.riced/Cargo.lock` for reproducible builds. The generated
  `Cargo.toml` and `src/main.rs` are deterministic and can be regenerated;
  `target/` is build output.
- `--framework PATH` or `RICED_FRAMEWORK` selects the framework source. The
  default is the source tree this `riced` binary was built from, which may not
  exist for a packaged install.
- Changing Rust extension code requires rebuilding and restarting. Lua app
  reloads do not rebuild extensions.
- The first build in a new target directory also builds framework dependencies;
  later builds reuse them. Lucide icons come from the pinned, checked-in SVG
  catalog and require no build-time download.

Registration rejects incompatible SDK versions, duplicate extension/service
names, built-in topic collisions, invalid identifiers, duplicate commands, and
oversized initial snapshots. An extension is registered atomically. Names must
start with a lowercase ASCII letter and contain only lowercase ASCII letters,
digits, or `_`, at most 64 bytes. Service names are flat; built-in dotted topics
such as `wayland.outputs` keep their existing behavior. At most 64 services and
64 commands per service are accepted.

Service constructors and `initial_snapshot` must be quick and avoid blocking I/O:
they run at registry initialization. Rendering and subscriptions read cached data
only. Extensions do not install new Lua globals or receive Lua/shell references.

## Lua service commands

```lua
function app:mount()
    self.palette = riced.service("palette")
    self.palette:subscribe(function(snapshot)
        self.colors = snapshot.data.colors
    end)
    self.palette:command("generate", {
        seed = "#89b4fa", -- exactly #rrggbb
        dark = true,     -- optional, defaults to false
    }, function(result)
        if result.ok then
            self.colors = result.data.colors
        else
            self.error = result.error.message
        end
    end)
end
```

`service:snapshot()` returns detached `{name, available, data}` snapshots, and
`service:subscribe(callback)` uses the existing coalesced managed subscriptions.
`service:command(name, args, callback)` returns a token for `self:cancel(token)`.
Commands can start in mount, actions, ticks, or managed callbacks, not views or
unmount. Unknown services/commands and invalid transport arguments fail before
scheduling. Existing built-in services remain read-only through this interface.
Service-specific option errors are asynchronous command failures.

Success is `{ok=true, data=...}` with an optional replacement `snapshot`.
Failure is `{ok=false, error={code, message}}`. Callbacks run on the host update
thread. Snapshots are committed before invoking the callback, only after the
owning generation/token is accepted. Cancellation, teardown, and reload suppress
late callbacks **and** snapshot publication. A callback error does not roll back
an already accepted snapshot. Concurrent commands commit in accepted completion
order, not submission order; serialize calls when ordering matters. Subscriptions
observe changes on the next host refresh, coalescing equal snapshots as usual.

### Unsolicited updates

A service may publish state without a command. Implement `Service::refresh` to
return `Some(snapshot)` only when the published snapshot changed; the host polls
it on its existing 500 ms service tick, validates and publishes changed values,
and repaints. This is how a long-lived service keeps its own listener resources
and exposes their latest state. `refresh` runs on the host update thread, so it
must be quick and non-blocking — never do I/O there. The default implementation
reports no change, so request/response services are unaffected. A service error
aborts that tick; the host logs it and retries.

Subscriptions observe the new snapshot on the next refresh, exactly like a
command-committed snapshot.

Supervised process services support the same model. A worker implements
`Service::refresh`; the host polls it on demand through the protocol's
`refresh`/`snapshot` exchange (see below). The host only polls a worker when it
actually reads the service, never on a timer of its own, so an idle worker with
no readers is not contacted.

## Compiled-in execution and limits

- Four blocking workers at most; the worker retains its permit until it exits,
  even if its owning app cancels.
- Commands/results: 64 KiB each. Snapshots: 1 MiB. Both: maximum depth 32 and
  16,384 JSON value nodes. Existing per-app managed resource/queue limits apply.
- `Cancellation` is cooperative. Running Rust code cannot be forcibly stopped;
  check the token during expensive operations. There is no command timeout in
  this milestone. Uncooperative work can occupy a worker indefinitely.
- Return replacement state through `Output.snapshot`; do not mutate published
  state from worker code. Cancellation cannot undo external I/O or other side
  effects performed by a trusted native extension.
- Worker panics become `worker` errors with unwinding builds; process aborts and
  arbitrary native bugs are not isolated. These crates are trusted code, not a
  security sandbox. Use the process backend below for crash isolation.

## Supervised executable services

The SDK's optional `process` feature adds a Tokio-based supervisor and the
`process::serve(&service)` worker entry point. The default SDK remains runtime
independent. `extensions/palette-worker` runs the same palette service in a
separate executable; its Lua API is unchanged.

Build the worker separately, then enable it **instead of** the compiled-in
palette registration in the generated project:

```sh
cargo build -p riced-extension-palette-worker --locked
```

```lua
return {
    extensions = {
        { name = "palette_worker", path = "../extensions/palette-worker", process = {
            argv = { "/absolute/path/to/riced/target/debug/riced-extension-palette-worker" },
            service = "palette",
            commands = { "generate" },
            timeout_ms = 30000,
        } },
    },
    apps = { { name = "palette" } }, -- copy the existing palette sample
    bar = {},
}
```

Run `riced build -p PROJECT`, then start the generated shell. Process entries
add no worker Cargo dependency to the shell. `argv` is an explicit program plus
arguments, never a shell string; the working directory is the project root. Use
an absolute executable path or a project-relative path containing `/`; plain
executable names use `PATH`. Workers are **not** built or downloaded at
runtime. Configuration is captured in the generated shell, so registration
changes require rebuilding; replacing an executable takes effect when that
worker next starts.

### Building extension crates

`path` on a process entry is optional and only tells `riced extension build`
where the crate lives; it does not link the worker into the shell. Omitting it
falls back to `extensions/<name>` beside the project.

```sh
riced extension build hello            # compile one extension crate
riced extension build                  # compile every extension in the project
riced extension update                 # compile everything, then rebuild the shell
riced extension build hello --release  # optimized
riced extension update --locked        # require the recorded lockfiles
```

`build` compiles the selected crate(s) with the workspace `cargo` and stops
there. `update` compiles first, then runs `riced build` so compiled-in changes
are linked into a fresh shell. Process workers only need their own binary
rebuilt and a shell restart, so `update` is the one command that works for
both kinds.

A selection of executable extensions only skips the shell rebuild: nothing is
linked, so `riced extension update hello` compiles the worker and stops.
Rebuilding the shell needs the same native build dependencies as building riced
itself (pkg-config, wayland, xkbcommon, vulkan), while `riced extension build`
does not. `examples/extensions/hello` is a complete, buildable process
extension and `examples/config.lua` enables it.

### Ownership and restart policy

- Registration does no process I/O. The supervisor and child start lazily on
  the first command and are shared by all apps using that service.
- One command runs at a time, with one queued command. Additional calls fail
  with `busy`. Existing host-wide worker and per-app limits still apply.
- `timeout_ms` defaults to 30,000 (range 10..300,000). The deadline includes
  queueing, restart backoff, startup, handshake, pipe writes, and response reads.
- Cancellation of an active request, timeout, disconnect, invalid frames, or
  mismatched IDs kills and reaps the direct child. Cancelled queued requests
  are skipped without killing another app's active command.
- Failed commands are **never replayed**. A later command starts a fresh worker
  after a 250 ms failure backoff; there is no automatic crash/restart loop.
- Initially `snapshot()` is `{name, available=false, data={}}`. Availability
  becomes true after negotiation and false on detected failure or shutdown.
  Idle exits are checked every 50 ms. Cached accepted data survives restarts.
  Existing subscriptions observe data/availability changes on the next host
  refresh; neither reads nor subscriptions launch workers.
- Snapshots are still committed only after live owner/token acceptance. A
  worker's unsolicited state is delivered through the `refresh`/`snapshot`
  exchange: the host asks the worker for its latest snapshot only while the
  service is being read, and only when no command is in flight, so frames never
  interleave. A refresh error or timeout is treated like a failed command and
  drops the worker. An app reload cancels its outstanding commands, not an
  otherwise idle shared service. Worker-local state is not restored after
  restart; persist important state explicitly.
- Normal host return and registry shutdown stop/reap children. This is not an
  OS sandbox or a general process-tree manager: workers must not spawn detached
  descendants. Forced host termination is not a guaranteed cleanup path.
  Native side effects remain nontransactional even when delivery is cancelled.

### Protocol v2

Stdout carries one UTF-8 JSON object per newline; stderr is inherited for logs.
Handshake:

```json
{"type":"hello","version":2}
{"type":"ready","version":2,"service":"palette","commands":["generate"]}
```

The worker's service and ordered commands must exactly match registration.
Requests and responses:

```json
{"type":"command","id":1,"method":"generate","args":{"seed":"#89b4fa"}}
{"type":"success","id":1,"output":{"data":{},"snapshot":null}}
{"type":"failure","id":1,"error":{"code":"seed","message":"invalid seed"}}
```

Unsolicited state uses an id-matched exchange. The host sends `refresh`; the
worker runs `Service::refresh` and replies with `snapshot`, where a `null`
snapshot means no change. A refresh error is returned as a normal `failure`:

```json
{"type":"refresh","id":2}
{"type":"snapshot","id":2,"snapshot":{"colors":{}}}
{"type":"snapshot","id":3,"snapshot":null}
```

Exactly one response is allowed for each command. IDs increase within a worker
session. Unknown fields and wrong frame types fail negotiation/execution.
Command data and snapshots retain the SDK's byte/depth/node limits. The frame
buffer is bounded to 1 MiB + 64 KiB + 4096 bytes, plus its newline. Error codes
are limited to 256 bytes and messages to 4096 bytes. No wire cancellation frame
is needed: the host terminates unresponsive workers. The SDK helper exits when
stdin closes; keep stdout free of ordinary printing.

## Palette example

The [framework example](../examples/README.md) enables the supervised
palette worker and includes build/run instructions. Its Lua palette card also
works with the compiled-in backend below.

`extensions/palette` implements `palette.generate`. It produces deterministic
Material colors from a seed, returning `{ready, seed, dark, colors}` and replacing
the shared snapshot with the same data. The initial snapshot is `{ready=false}`.
It does not apply the shell theme, extract wallpaper colors, write preferences,
or process templates. Lua decides how to present or persist its output.

`colors` contains all 22 roles accepted by the built-in `iced.palette` API,
including `on_*` foregrounds. The service snapshot is generated-preview state,
not the shell's applied palette. Read applied colors with `iced.palette()`.
The example card provides separate **Generate preview** and **Apply generated
palette** buttons. An explicit apply action returns `iced.palette(self.colors)`.
To generate and apply in one operation instead:

```lua
self.palette:command("generate", { seed = "#89b4fa", dark = true }, function(result)
    if result.ok then return iced.palette(result.data.colors) end
    self.error = result.error.message
end)
```

The returned command is validated and applied by the host update thread, only
after the callback's owner/generation is accepted. It persists color overrides
without changing named theme files or running external-app template hooks.
No new SDK capability or direct worker access to the shell/Lua state is needed.

Copy `examples/apps/palette/` into your project's `apps/` directory
and `extensions/palette/` into `extensions/`, enable `{ name = "palette" }` in
`config.lua`, then run `riced build` to display the optional sample.

## Theme generation example

`extensions/theme-gen` implements `theme_gen.generate`, taking
`{seed, variant?, darkmode?, image?}` and returning
`{payload = {light, dark}, variables = {...}}` — the reshell-format scheme JSON
and the `{{var}}` template variables. The initial snapshot is `{ready=false}`;
a successful command replaces it with `{ready, seed, variant, darkmode}`.

This crate also owns the pure scheme and color-map logic that the host's
`theme_gen` module re-exports, so the in-tree `riced generate-theme` pipeline
and the service share one implementation. The crate never samples wallpaper
pixels or writes files: `riced generate-theme` keeps wallpaper sampling,
`dynamic.json` writes, and template processing, and calls the shared code for
the scheme itself.

`examples/apps/theme_gen/` is the sample card; enable
`{ name = "theme_gen", path = "../../extensions/theme-gen" }` (paths are
relative to the project root) and add the app entry, then rebuild the shell.

## Wayland toplevels and workspaces examples

Two crates own their own Wayland connection and expose a command-less service
whose snapshot changes as the compositor reports updates:

- `extensions/wayland-toplevels` — `ext-foreign-toplevel-list-v1`; the
  `toplevels` service publishes `{toplevels = [{app_id, title}, …]}` as windows
  map, retitle, and close.
- `extensions/wayland-workspaces` — `ext-workspace-v1` (+ `xdg-output-v1`); the
  `workspaces` service publishes `{workspaces = [{name, monitor, active}, …]}`.
  `monitor` is the compositor-reported name, not the host's geometry-resolved
  one.

Each listener thread blocks on the compositor socket and wakes the host only on
change; compositors without the protocol yield an empty list, and a revoked list
clears it. The host's `services::toplevels` and `services::workspaces` re-export
`ToplevelCache`/`WorkspaceCache` and only adapt the host's coalesced wakeup, so
each core `wayland.*` table and its extension service share one implementation.

Neither extension is in the default registry — a project that enables them
compiled-in gets one extra Wayland connection per service, alongside the core
ones. Consolidating them is tracked in the migration plan.

`examples/apps/toplevels/` and `.../workspaces/` are the sample cards.
Both services have no commands, so the supervised-process backend is not usable
for them yet: workers start lazily on the first command, and a push-only service
has none.

Run SDK, palette, theme-gen, wayland-toplevels, wayland-workspaces, and host
checks with `cargo test --workspace --locked`.
