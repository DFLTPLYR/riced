# riced

Wayland layer-shell daemon built with [`iced` 0.14](https://github.com/iced-rs/iced) + [`iced_exwlshell` 0.20](https://github.com/wayland-rs/iced_exwlshell) — per-output fullscreen backgrounds with global drag-selection, context menu, and top bars. Ported from a Quickshell `Background` + `selectionRect` QML singleton.

## Features

- **Layer shell daemon** (`daemon` + `exwlshell`) — one `Background` (`Layer::Background`, `Anchor::all`, `Size::FILL`) + one `Top` bar (`50px`, `Anchor::Top`, `exclusive_zone`) per output via `OutputInsert`
- **Global selectionRect** — `QtObject { startPoint, selecting, x/y/width/height, onSelectingChanged { if(!selecting) width=0 height=0 startPoint=null } }` → `SelectionRect` `src/app/app.rs:17` with `set_selecting` `150ms InOutQuad` fade, `to_global` `mapToGlobal` via `OutputInfo.logical_position` `src/app/app.rs:212`
- **Cross-monitor drag** — global `x/y/w/h` clipped per `panel.screen` `min(x+w,sx+sw)-max(x,sx)` `src/app/app.rs:267` + `Utils.intersects` `src/app/app.rs:225`, `Scope::All` on `SelectionTick` `src/app/app.rs:591`
- **Smooth input** — `hoverEnabled: selecting` via `AtomicBool SELECTING` `src/app/app.rs:105`, `CursorMoved` throttled at source `16ms ~60fps` `src/app/app.rs:107` + `16ms` in `update` `src/app/app.rs:490`, filtered to `Event::Mouse` only (drops `RedrawRequested` flood), `SelectionTick` drives `All` redraw; `canvas` `SelCanvas` `src/app/app.rs:52` single `fill/stroke` vs `container+stack` layout
- **Context menu** — `Right` `ButtonPressed` `src/app/app.rs:525` `contextMenu {x,y,open}` `src/app/app.rs:42`, clipped to screen, `Left` press closes

## Requirements

- Wayland compositor with `wlr-layer-shell` (Hyprland, Sway, Niri, etc.)
- `wayland-client` connection (`WAYLAND_DISPLAY`)
- `libxkbcommon` (`xkbcommon.pc` on `PKG_CONFIG_PATH`) — Nix provides it
- Rust `>=1.85` (edition `2024`), `cargo` with `tokio`, `wgpu`/`tiny-skia`, `canvas` features `Cargo.toml:7`

## Install & Run

```bash
# Nix (recommended, provides xkbcommon, wayland)
nix develop # or direnv
cargo run --release              # release ~3× smoother than debug (wgpu validation off)
RUST_LOG=warn cargo run --release # suppress info flood, keeps WARN tracker
# or
PKG_CONFIG_PATH=/nix/store/...-libxkbcommon-*/lib/pkgconfig cargo run --release

# debug (choppy, for logs)
cargo run
RUST_LOG=warn cargo run
```

`cargo run --release` parses `Settings` `src/main.rs:24` — `RUST_LOG=warn cargo run --release` is correct (`RUST_LOG=warn` is env, not arg).

## Usage

Left-drag on any `Background` (fullscreen) starts `selecting`:

```
select start Point { x, y } (local Point { x, y })  # src/app/app.rs:554
select end -> reset rect (fade 150ms)                # src/app/app.rs:567
```

- `Left` `MouseArea.anchors.fill parent` `src/app/app.rs:354` `propagateComposedEvents:true` → `selecting=true startPoint=mapToGlobal(mouse.x,mouse.y)` `src/app/app.rs:543`
- `CursorMoved` while `selecting` → `min/max` `src/app/app.rs:500` `width = max-min`
- `Left Released` → `selecting=false` → `width=0 height=0 startPoint=None` `src/app/app.rs:29` + `fade_rect` `150ms` `opacity 1-eased` `src/app/app.rs:251`
- `Right` → `contextMenu` at `x/y` `src/app/app.rs:525`, next `Left` closes

`Top` bar `src/app/layers/top.rs:14` is per-output `Top` layer, not selectable.

## Architecture

```
src/main.rs:10          daemon(Plots::new, namespace, update, view)
                        .subscription(Plots::subscription) // mouse throttled + shell
                        .settings(Settings { layer_settings: Active 1px placeholder })
                        // per-output Background+Top spawned via Plant::NewLayerShell on OutputInsert
src/app/mod.rs:1        app, events, layers
src/app/events.rs:13    Plant { Grow, Uproot(Id), Tend, Wayland(WayEvent), Graft(Id,Event), SelectionTick } #[to_layer_message(multi)]
src/app/app.rs:17       SelectionRect, ContextMenu, SelCanvas (canvas::Program), Plots { ids, tops/top_ids, backgrounds/background_ids, output_infos, last_cursor, selection_rect/fade, last_selection_tick }
src/app/layers/background.rs:12  Background::open(GlobalName) FILL Anchor::all
src/app/layers/top.rs:14        Top::open(GlobalName) fill_width(50) Anchor::Top
src/app/app.rs:99       subscription: listen_with(throttled_graft) + shell_events + time::every(16ms) while fading
src/app/app.rs:454      redraw_scope: SelectionTick|Button => All, CursorMoved => None
```

## Configuration

`src/main.rs:24` `LayerShellSettings`:

```rust
anchor: Anchor::Top, size: LayerSize::fill_width(1), exclusive_zone: 0, start_mode: Active // daemon tiny
// per-output Background: Anchor::all Size::FILL Background (src/app/layers/background.rs:12)
// per-output Top: Anchor::Top Size::fill_width(50) exclusive_zone 50 Top (src/app/layers/top.rs:22)
```

`exclusive_zone: i32` not `LayerSize` `src/settings.rs:92`.

## Development

```bash
cargo check   # with PKG_CONFIG_PATH for xkbcommon
cargo fmt
cargo clippy
```

Flake `flake.nix` provides devShell. `Session.vim` for vim.

## Troubleshooting

- `xkbcommon.pc not found` → `PKG_CONFIG_PATH=/nix/store/...-libxkbcommon-*/lib/pkgconfig`
- `TrySendError Full` `WARN tracker` → `CursorMoved` flood, now throttled `src/app/app.rs:107` + ` Event::Mouse` filter `src/app/app.rs:189`
- `left lines on shrink` → was `canvas stroke 0.5px` + `stack` child swap `Space` `src/app/app.rs:279`; now `container Rectangle` `background+border` `src/app/app.rs:285` always `Fill`
- `startPoint random` → `last_cursor` stale when `!selecting`; now `LAST_CURSOR_GLOBAL` `src/app/app.rs:49` updated at source even when `None`
- Entire app laggy → `dev` `wgpu` validation + `println!` per drag + `Scope::All` per pixel; `release` + `RUST_LOG=warn` + `60fps` throttle fixes

## License

MIT (or as `Cargo.toml` specifies)
