{
  description = "Development environment for exwlshelleventloop";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
  }:
    flake-utils.lib.eachDefaultSystem (
      system: let
        pkgs = import nixpkgs {
          inherit system;
        };
      in {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            pkg-config
            wayland
            libxkbcommon
            libinput
            gcc
            vulkan-loader
            vulkan-tools
            dbus
          ];

          shellHook = ''
            # Wayland and the renderer load these libraries dynamically at runtime.
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [pkgs.wayland pkgs.libxkbcommon pkgs.vulkan-loader pkgs.libinput pkgs.dbus]}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
            echo "exwlshelleventloop dev shell"
            echo "Wayland:   $(pkg-config --modversion wayland-client)"
            echo "XKBCommon: $(pkg-config --modversion xkbcommon)"
            echo "Vulkan:    $(pkg-config --modversion vulkan)"
          '';
        };
      }
    );
}
