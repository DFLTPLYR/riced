# Default: list recipes
default:
    @just --list

# Build the release binary (inside the flake dev shell:
# system libs like xkbcommon only resolve via its pkg-config).
# Also embeds the nix store lib dirs as rpath: wayland-client dlopens
# libwayland-client at runtime, and wgpu dlopens libvulkan, so the installed
# binary must find both without LD_LIBRARY_PATH from the dev shell.
# (Without libvulkan no adapter is found and iced falls back to CPU
# tiny-skia/softbuffer, whose XRGB buffers strip alpha entirely.)
build:
    nix develop --command bash -c 'export RUSTFLAGS="-C link-args=-Wl,-rpath,$(pkg-config --variable=libdir wayland-client),-rpath,$(pkg-config --variable=libdir xkbcommon),-rpath,$(pkg-config --variable=libdir vulkan)"; cargo build --release'

# Build release and install to ~/.local/bin (already on PATH)
install: build
    install -Dm755 "target/release/riced" "{{ home_directory() }}/.local/bin/riced"

# Remove the installed binary
uninstall:
    rm -f "{{ home_directory() }}/.local/bin/riced"
