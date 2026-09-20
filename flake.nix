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
          ];

          shellHook = ''
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [
              pkgs.wayland
              pkgs.libxkbcommon
            ]}:$LD_LIBRARY_PATH"

            echo "exwlshelleventloop dev shell"
            echo "Wayland:   $(pkg-config --modversion wayland-client)"
            echo "XKBCommon: $(pkg-config --modversion xkbcommon)"
          '';
        };
      }
    );
}
