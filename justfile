nix := "nix develop --command"

# Default: list recipes
default:
    @just --list

# Required checks for structural changes (uses the same system libraries as build).
check:
    {{nix}} cargo fmt --all -- --check
    {{nix}} cargo test --workspace
    {{nix}} cargo clippy --workspace --all-targets
    git diff --check

# Build release inside the flake dev shell with runtime library paths.
build:
    #!/usr/bin/env bash
    {{nix}} bash <<'BUILD'
        set -euo pipefail

        # Embed Nix library paths for runtime loading outside the dev shell.
        # Missing Vulkan forces iced's CPU fallback, which strips alpha.
        rpath() {
            pkg-config --variable=libdir "$1"
        }

        rpaths="-Wl,-rpath,$(rpath wayland-client)"
        rpaths+=",-rpath,$(rpath xkbcommon)"
        rpaths+=",-rpath,$(rpath vulkan)"

        export RUSTFLAGS="-C link-args=$rpaths"
        cargo build --release
    BUILD
    install -Dm755 "target/release/riced" "{{ home_directory() }}/.local/bin/riced"

# Build release and install to ~/.local/bin (already on PATH)
install: build
    install -Dm755 "target/release/riced" "{{ home_directory() }}/.local/bin/riced"
    -pkill riced
    riced > /dev/null 2>&1 & disown

# Remove the installed binary
uninstall:
    rm -f "{{ home_directory() }}/.local/bin/riced"
