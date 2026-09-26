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
            # Vulkan loader: wgpu dlopens libvulkan at runtime. Without this
            # (and its rpath, see justfile) no adapter is found and iced falls
            # back to CPU tiny-skia/softbuffer, whose XRGB buffers strip alpha
            # and make transparency/blur impossible.
            vulkan-loader
          ];

          shellHook = ''
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [
              pkgs.wayland
              pkgs.libxkbcommon
              pkgs.vulkan-loader
            ]}:$LD_LIBRARY_PATH"

            echo "exwlshelleventloop dev shell"
            echo "Wayland:   $(pkg-config --modversion wayland-client)"
            echo "XKBCommon: $(pkg-config --modversion xkbcommon)"
            echo "Vulkan:    $(pkg-config --modversion vulkan)"
          '';
        };
      }
    );
}
