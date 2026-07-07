{
  description = "waverunner — rofi-like Wayland launcher (dev shell)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems
        (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer
            pkg-config
          ];

          buildInputs = with pkgs; [
            wayland
            libxkbcommon
            vulkan-loader
          ];

          # wgpu loads libvulkan.so / libwayland-client.so at runtime via
          # dlopen; on NixOS they are not in the default search path.
          LD_LIBRARY_PATH = nixpkgs.lib.makeLibraryPath (with pkgs; [
            wayland
            libxkbcommon
            vulkan-loader
          ]);

          RUST_LOG = "waverunner=debug";
        };
      });
    };
}
