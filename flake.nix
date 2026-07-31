{
  description = "waverunner — Wayland dock and launcher";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, home-manager }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # Runtime libs wgpu dlopens; must be on LD_LIBRARY_PATH at process start.
      runtimeLibs = pkgs: with pkgs; [ wayland vulkan-loader libxkbcommon ];

      # External tools the daemon shells out to for Files-section thumbnails:
      # ffmpegthumbnailer for videos, poppler's pdftoppm for PDFs. Missing
      # ones just fall back to the file's type icon.
      runtimeTools = pkgs: with pkgs; [ ffmpegthumbnailer poppler-utils ];
    in {

      # ── Nix packages ────────────────────────────────────────────────────────
      packages = forAllSystems (pkgs: rec {
        waverunner-daemon = pkgs.rustPlatform.buildRustPackage {
          pname = "waverunner-daemon";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          buildInputs = with pkgs; [ wayland libxkbcommon vulkan-loader ];
          nativeBuildInputs = with pkgs; [ pkg-config makeWrapper ];

          cargoBuildFlags = [ "-p" "waverunner-daemon" ];
          # Tests that exercise Hyprland IPC need a live session; skip here.
          doCheck = false;

          # wgpu uses dlopen for Vulkan/Wayland; wrap so store paths are found.
          # Also put the thumbnailer tools on PATH so previews work whatever
          # the launching environment.
          postInstall = ''
            wrapProgram $out/bin/waverunner \
              --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath (runtimeLibs pkgs)} \
              --prefix PATH : ${pkgs.lib.makeBinPath (runtimeTools pkgs)}
          '';
        };

        waverunner-ctl = pkgs.rustPlatform.buildRustPackage {
          pname = "waverunner-ctl";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = with pkgs; [ pkg-config ];
          cargoBuildFlags = [ "-p" "waverunner-client" ];
          doCheck = false;
        };

        # The OPTIONS notification daemon (org.freedesktop.Notifications server).
        # Pure Rust (zbus/tokio) — no wgpu/wayland runtime libs needed.
        options-notify = pkgs.rustPlatform.buildRustPackage {
          pname = "options-notify";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "-p" "options-notify" ];
          doCheck = false;
        };

        default = waverunner-daemon;
      });

      # ── Home-manager module ──────────────────────────────────────────────────
      #
      # Usage in your home.nix:
      #
      #   imports = [
      #     waverunner.homeManagerModules.default
      #     ./waverunner-packages.nix   # launcher-managed install list
      #   ];
      #   programs.waverunner.enable = true;
      #
      homeManagerModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.programs.waverunner;
          sys = pkgs.stdenv.hostPlatform.system;
          defaultPkg = self.packages.${sys}.waverunner-daemon;
          defaultCtl = self.packages.${sys}.waverunner-ctl;
        in {
          options.programs.waverunner = {
            enable = lib.mkEnableOption "waverunner dock launcher";

            package = lib.mkOption {
              type = lib.types.package;
              default = defaultPkg;
              description = "The waverunner-daemon package to use.";
            };

            ctlPackage = lib.mkOption {
              type = lib.types.package;
              default = defaultCtl;
              description = "The waverunner-ctl package to use.";
            };
          };

          config = lib.mkIf cfg.enable {
            home.packages = [ cfg.package cfg.ctlPackage ];

            # Autostart the daemon when the graphical session begins.
            systemd.user.services.waverunner = {
              Unit = {
                Description = "waverunner dock launcher daemon";
                After    = [ "graphical-session.target" ];
                PartOf   = [ "graphical-session.target" ];
              };
              Service = {
                ExecStart = "${cfg.package}/bin/waverunner";
                Restart   = "on-failure";
                RestartSec = "1s";
              };
              Install.WantedBy = [ "graphical-session.target" ];
            };
          };
        };

      # ── Dev shell ────────────────────────────────────────────────────────────
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer
            pkg-config
            home-manager.packages.${pkgs.stdenv.hostPlatform.system}.home-manager
          ] ++ (runtimeTools pkgs);

          buildInputs = with pkgs; [ wayland libxkbcommon vulkan-loader ];

          # wgpu loads libvulkan.so / libwayland-client.so at runtime via
          # dlopen; on NixOS they are not in the default search path.
          LD_LIBRARY_PATH =
            nixpkgs.lib.makeLibraryPath (runtimeLibs pkgs);

          RUST_LOG = "waverunner=debug";
        };
      });
    };
}
