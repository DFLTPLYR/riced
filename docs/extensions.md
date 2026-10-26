# Rust extensions (experimental SDK v1)

Extensions are ordinary Cargo library crates, usually under
`extensions/<name>/`. `crates/riced-extension-sdk` is version `0.1.0`, with
source contract `SDK_VERSION = 1`. It depends on neither Lua nor iced. This is
not a stable Rust ABI: compiled-in extensions use path dependencies and require
rebuilding and restarting riced. No dynamic `.so` loading, runtime directory
scanning, or supervised extension processes are implemented.

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
palette extension for the bundled reference shell. Project shells are built
from the extensions enabled in `config.lua`; no framework source edits are
needed. Extensions are session-wide, not per project/output, and do not
restart when an app reloads.

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
- The first build in a new target directory also builds dependencies such as
  `lucide-iced`; later builds reuse them.

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

## Execution and limits

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
  security sandbox. Supervised processes remain a later milestone.

## Palette example

`extensions/palette` implements `palette.generate`. It produces deterministic
Material colors from a seed, returning `{ready, seed, dark, colors}` and replacing
the shared snapshot with the same data. The initial snapshot is `{ready=false}`.
It does not apply the shell theme, extract wallpaper colors, write preferences,
or process templates. Lua decides how to present or persist its output.

Copy `examples/framework/apps/palette/` into your project's `apps/` directory
and `extensions/palette/` into `extensions/`, enable `{ name = "palette" }` in
`config.lua`, then run `riced build` to display the optional sample.

Run SDK, palette, and host checks with `cargo test --workspace --locked`.
