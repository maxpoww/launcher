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

      # External tools the daemon shells out to. Everything degrades gracefully
      # when absent, but "gracefully" can mean a whole OPTION silently doing
      # nothing on a machine that lacks the tool — so ship them all:
      #  - wl-clipboard (wl-paste/wl-copy): the clipboard OPTION's capture/serve
      #  - grim: link-clip window snapshots (share-card hero)
      #  - curl: opt-in link unfurl + Flathub/store icon fetch
      #  - ffmpegthumbnailer / poppler's pdftoppm: Files-section thumbnails
      #  - nix-index (nix-locate): icon/desktop hints for the package index
      #    without `nix shell nixpkgs#nix-index` (a ~3GB nixpkgs eval)
      runtimeTools = pkgs: with pkgs; [ wl-clipboard grim curl ffmpegthumbnailer poppler-utils nix-index ];
    in {

      # ── Nix packages ────────────────────────────────────────────────────────
      packages = forAllSystems (pkgs: rec {
        # Offline dictionary data for the clipboard "define a word" panel, built
        # declaratively from pinned upstream sources: Webster's 1913 (English,
        # public domain) copied as-is, and the RAE dump (Spanish) parsed by
        # tools/rae-parse into `{word: {e,d}}` JSON. Installed to
        # $out/share/waverunner/, pointed at by $WAVERUNNER_DICT[_ES].
        dictionaries =
          let
            english = pkgs.fetchurl {
              url = "https://raw.githubusercontent.com/matthewreagan/WebstersEnglishDictionary/6fb9c92420c3a323e74ffb9d577409f4431cc42a/dictionary_compact.json";
              hash = "sha256-FrEoR6R8wSAuXkCjpEue0vdJ0jwKXwrRH8vU6MbD4z0=";
            };
            raeSource = pkgs.fetchurl {
              url = "https://raw.githubusercontent.com/eneko98/RAE-Corpus/7cc61043a0a6379108ced0a83c77d9dbdbfe0835/RealAcademiaEspanola-DiccionarioLlenguaEspanola.txt";
              hash = "sha256-ZWxY8mExCM9i/SQIkfVHoQfAIKTYX/fYqHpu1eUz0ik=";
            };
          in
          pkgs.stdenv.mkDerivation {
            pname = "waverunner-dictionaries";
            version = "0.1.0";
            dontUnpack = true;
            # rustc (with the stdenv `cc` for linking) compiles the single-file,
            # dependency-free parser; no cargo/workspace needed.
            nativeBuildInputs = [ pkgs.rustc ];
            buildPhase = ''
              runHook preBuild
              rustc -O --edition 2021 ${./tools/rae-parse/parse_rae.rs} -o parse_rae
              ./parse_rae ${raeSource} dictionary-es.json
              runHook postBuild
            '';
            installPhase = ''
              runHook preInstall
              mkdir -p $out/share/waverunner
              cp ${english} $out/share/waverunner/dictionary.json
              cp dictionary-es.json $out/share/waverunner/dictionary-es.json
              runHook postInstall
            '';
          };

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
          # Also put the thumbnailer tools on PATH so previews work whatever the
          # launching environment, and point the dictionary panel at the
          # declaratively-built data (a user's own env override still wins).
          postInstall = ''
            wrapProgram $out/bin/waverunner \
              --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath (runtimeLibs pkgs)} \
              --prefix PATH : ${pkgs.lib.makeBinPath (runtimeTools pkgs)} \
              --set-default WAVERUNNER_DICT ${dictionaries}/share/waverunner/dictionary.json \
              --set-default WAVERUNNER_DICT_ES ${dictionaries}/share/waverunner/dictionary-es.json
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

        # The nixpkgs package index, prebuilt from THIS flake's pinned
        # nixpkgs (a distro flake's `follows` makes it match exactly what
        # the system installs from). The daemon loads it instantly via
        # WAVERUNNER_PKG_INDEX (the home-manager module wires this) instead
        # of running the ~3GB `nix search nixpkgs ^` eval at cold start —
        # which OOM-crash-looped the whole shell on a 4G machine
        # (2026-08-30). No icon hints at build time (nix-locate needs its
        # downloaded database); the runtime flathub/store fetchers cover
        # icons lazily.
        package-index = pkgs.runCommand "waverunner-package-index"
          {
            nativeBuildInputs = [ pkgs.nix pkgs.jq waverunner-daemon ];
          } ''
          export HOME=$TMPDIR NIX_STATE_DIR=$TMPDIR/nix
          nix-env -f ${nixpkgs} -qa --json --meta \
            --arg config '{ allowUnfree = true; }' > raw.json
          jq --arg sys ${pkgs.stdenv.hostPlatform.system} '
            to_entries
            | map({ key: ("legacyPackages." + $sys + "." + .key),
                    value: { pname: (.value.pname // ""),
                             version: (.value.version // ""),
                             description: (.value.meta.description // "") } })
            | from_entries' raw.json > search.json
          waverunner build-index search.json \
            $out/share/waverunner/nixpkgs-index.tsv
        '';

        default = waverunner-daemon;
      });

      # ── NixOS module ─────────────────────────────────────────────────────────
      #
      # Makes OPTIONS the system notification daemon. Usage in your NixOS flake:
      #
      #   imports = [ waverunner.nixosModules.notification-service ];
      #   services.options-notify.enable = true;   # package defaults to ours
      #
      nixosModules.notification-service = { pkgs, lib, ... }: {
        imports = [ ./notification-service.nix ];
        services.options-notify.package =
          lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.options-notify;
      };

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
          defaultIdx = self.packages.${sys}.package-index;
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

            packageIndex = lib.mkOption {
              type = lib.types.nullOr lib.types.package;
              default = defaultIdx;
              description = ''
                Prebuilt nixpkgs package index (WAVERUNNER_PKG_INDEX).
                null = the daemon dumps its own index at runtime — a ~3GB
                nixpkgs eval; never do that on small machines.
              '';
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
              } // lib.optionalAttrs (cfg.packageIndex != null) {
                Environment = [
                  "WAVERUNNER_PKG_INDEX=${cfg.packageIndex}/share/waverunner/nixpkgs-index.tsv"
                ];
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
