# Lua framework and process extension example

A Lua bar, palette card, theme-generation card, layout-editor card, hello
extension card, toplevels card, workspaces strip, and app-owned popup, with
palette and hello services running as supervised Rust executables and theme
generation plus the Wayland toplevel/workspace listeners compiled in from
`../crates/`. Run only one riced daemon per session; stop an existing one
before starting this example.

## Build and run

This project still commits the legacy layout (`config.lua` and `settings/`).
Before building, edit `config.lua`: change the four bundled extension paths
from `../extensions/` to `../crates/` (`theme-gen`, `wayland-toplevels`,
`wayland-workspaces`, and `palette-worker`). Keep the project-owned
`extensions/hello` path unchanged. Otherwise extension builds fail because the
old directories no longer exist. The commands below build/start a desktop;
workspace tests alone do not validate this generated-shell build.

On startup, the runtime imports preferences into `data/config.lua` and moves
Settings pages to `components/settings/`. Once imported, edit `data/config.lua`;
the original top-level config is kept but no longer wins.

From the **repository root**, build the CLI:

```sh
nix develop --command cargo build --workspace --locked
```

Compile the example's extension crates and build its shell:

```sh
nix develop --command ./target/debug/riced extension update -p ./examples
```

Start the generated binary (not the default `riced` binary):

```sh
nix develop --command ./examples/target/riced-shell/debug/riced-shell-examples
```

The generated shell already defaults to this project, so `-p` is only needed if
you move it. Add `--release` to the `extension update` line for a faster shell
at `examples/target/riced-shell/release/riced-shell-examples`. Outside Nix, omit
`nix develop --command` if the build/runtime dependencies are already available.
Later builds can pass `--locked` to require the recorded `.riced/Cargo.lock`
instead of updating it.

## What to expect

- A three-slot bar (start, center, end) on each output and a palette card near
  the top-right corner.
- An **Open menu** button near the bottom-left opens an app-owned popup. Its
  counter shares the button's Lua state. Close it with **Close**, Escape, or a
  click outside the card; other outputs keep independent state. See
  [app-owned popups](../docs/framework.md#app-owned-popups).
- The palette service starts unavailable with an empty cached snapshot. The
  card requests a palette on mount, starting the worker lazily.
- **Generate palette** requests another palette. Errors appear on the card;
  click again to retry after fixing a missing executable or a failed worker.
- The service is shared across outputs; subscriptions receive accepted palette
  changes without extension-specific Lua code.
- A worker crash does not terminate the shell. The next command starts a fresh
  worker; failed commands are not replayed automatically.
- A theme-generation card sits below the palette card. It calls the compiled-in
  `theme_gen` service, which builds the reshell `{light, dark}` scheme and
  template variables from a seed; the card only presents the result. **Next
  seed** cycles seeds. The service never samples wallpapers or writes files —
  `riced generate-theme` still does that.
- A toplevels card sits at the bottom-left. It subscribes to the compiled-in
  `toplevels` service (the `ext-foreign-toplevel-list-v1` listener in
  `../crates/wayland-toplevels`) and lists `{app_id, title}` for every mapped
  window. Compositors without the protocol show **No toplevels**.
- A workspaces strip sits at the top-left. It subscribes to the compiled-in
  `workspaces` service (`ext-workspace-v1` in `../crates/wayland-workspaces`)
  and highlights the active workspace. Compositors without the protocol show
  **No workspaces**. Each of these is an extra Wayland connection beyond the
  core `wayland.toplevels`/`wayland.workspaces` tables; a project only needs
  one of each.
- A layout-editor card sits at the bottom-right. Its `iced.spin_box` controls
  drive an `iced.masonry` card layout live, showing the numeric change event
  (`event.value`) and responsive layout without any native editor widget.
- A hello card sits at the top-left, below the workspaces strip. It consumes the
  project-owned `hello` process extension in `extensions/hello` and shows
  availability, the accepted greeting, and worker errors.

The configuration also sets the shell-level surface the example depends on:
`theme` (`gruvbox`, dark), `animation.speed`, `interaction.hold`, and
`notifications` (bundled `notifications.lua` scene on the mouse output).
Wallpaper and desktop widget placements are machine-specific, so `data/config.lua`
keeps them as commented templates.

`data/config.lua` resolves its executable path relative to this project directory:
`../target/debug/riced-extension-palette-worker`. Build that worker before
running the shell. `riced build` does **not** build process executables.

After editing worker Rust code, rebuild the worker and restart the example to
use the new executable. Lua apps retain their regular hot reload behavior.
Changing extension registration in `data/config.lua` requires rebuilding the
generated shell and restarting it.

## Building extensions

`extensions/hello/` is a project-owned process extension: one `hello` service
with a `greet` command, served over the SDK's stdio protocol. It is the sample
to copy when writing your own.

```sh
# Compile the crate(s) the project enables.
riced extension build hello -p ./examples
riced extension build -p ./examples

# Compile everything, then rebuild the generated shell so compiled-in
# changes (theme_gen, wayland_toplevels, wayland_workspaces) are linked.
riced extension update -p ./examples
```

`build` compiles the selected crate(s) with cargo and stops; `update` also runs
`riced build`. Process workers such as `hello` and `palette_worker` only need
their own binary rebuilt and a restart, so a process-only selection skips the
shell rebuild entirely: `riced extension update hello` compiles the worker and
stops. Each entry's `path` names its crate directory (`extensions/hello` for
`hello`, `../crates/palette-worker` for the palette worker); without `path`
the default is `extensions/<name>` beside the project. See
[Rust extensions](../docs/extensions.md#building-extension-crates).

Rebuilding the shell (`riced build`, and `update` when a compiled-in extension
is selected) needs the same native build dependencies as building riced itself:
pkg-config, wayland, xkbcommon, and vulkan. Inside this repository that means
running under `nix develop`, exactly like the initial build above. Compiling
extension crates alone does not need them.

## Project-owned Settings page

`settings/animation.lua` is the committed, project-owned Settings page. Startup
moves it to `components/settings/animation.lua` and adopts that directory as
user-owned, so other missing pages use bundled fallbacks. Open Settings and edit
the migrated file to see the page reload without restarting the shell; delete it
to fall back to the bundled
page. The host still supplies the page's `props` and validates its actions
(`animation:fast|medium|slow`), so only presentation changes here. See
[Lua API — Settings pages](../docs/lua-api.md#settings-pages).

The project's legacy `.gitignore` does not cover every new `data/` or
`composables/` path. Running it can move tracked files and create untracked
runtime data. Use a separate copy if you want to keep this checkout unchanged.

## Copying into another project

Copy `apps/`, enable the apps and process entries from the example config in
your own `data/config.lua`, and change
`process.argv[1]` to your worker's installed executable path. Set `path` to the
worker's crate directory so `riced extension build` can compile it. Relative
paths containing `/` resolve from that project's root; plain names use `PATH`.

For the compiled-in backend instead, replace the process entry with:

```lua
{ name = "palette", path = "/absolute/path/to/riced/crates/palette" },
```

Rebuild the shell after switching. Do not enable both backends simultaneously:
both register the `palette` service. The Lua palette app works with either.

See [Rust extensions](../docs/extensions.md) for protocol, cancellation,
deadlines, snapshot ownership, and process-isolation limits.
