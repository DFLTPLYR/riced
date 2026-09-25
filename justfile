# Default: list recipes
default:
    @just --list

# Build the release binary (inside the flake dev shell:
# system libs like xkbcommon only resolve via its pkg-config).
# Also embeds the nix store lib dirs as rpath: wayland-client dlopens
# libwayland-client at runtime, so the installed binary must find it
# without LD_LIBRARY_PATH from the dev shell.
build:
    nix develop --command bash -c 'export RUSTFLAGS="-C link-args=-Wl,-rpath,$(pkg-config --variable=libdir wayland-client),-rpath,$(pkg-config --variable=libdir xkbcommon)"; cargo build --release'

# Build release and install to ~/.local/bin (already on PATH)
install: build
    install -Dm755 "target/release/riced" "{{ home_directory() }}/.local/bin/riced"

# Remove the installed binary
uninstall:
    rm -f "{{ home_directory() }}/.local/bin/riced"
