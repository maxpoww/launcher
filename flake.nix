{
  description = "waverunner — Wayland dock and launcher";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # Prebuilt nix-index file database (weekly): lets package-index bake
    # desktop-file stems + icon paths at build time, offline. Without
    # them multi-desktop suites (libreoffice) can't resolve gui.
    nix-index-database = {
      url = "github:nix-community/nix-index-database";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, home-manager, nix-index-database }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # Runtime libs wgpu dlopens; must be on LD_LIBRARY_PATH at process start.
      # libglvnd is LOAD-BEARING: without libEGL.so.1 wgpu's GL backend cannot
      # initialize at all, and on GPUs whose Vulkan is incomplete (pre-Skylake
      # Intel — mesa offers only llvmpipe there) the shell silently fell back
      # to CPU rendering. Found live on a 2013 MacBook Air (menubox = 80% CPU,
      # whole shell sluggish); with GL reachable the very first adapter probe
      # picks the real iGPU (2026-09-02). The wrapper also needs the system's
      # driver dir at runtime — see the wrapProgram below.
      runtimeLibs = pkgs: with pkgs; [ wayland vulkan-loader libxkbcommon libglvnd ];

      # External tools the daemon shells out to. Everything degrades gracefully
      # when absent, but "gracefully" can mean a whole OPTION silently doing
      # nothing on a machine that lacks the tool — so ship them all:
      #  - wl-clipboard (wl-paste/wl-copy): the clipboard OPTION's capture/serve
      #  - grim: link-clip window snapshots (share-card hero)
      #  - curl: opt-in link unfurl + Flathub/store icon fetch
      #  - ffmpegthumbnailer / poppler's pdftoppm: Files-section thumbnails
      #  - nix-index (nix-locate): icon/desktop hints for the package index
      #    without `nix shell nixpkgs#nix-index` (a ~3GB nixpkgs eval)
      #  - pipewire (pw-record/pw-play): the card's voice notes and talk-to-text
      runtimeTools = pkgs: with pkgs; [ wl-clipboard grim curl ffmpegthumbnailer poppler-utils nix-index pipewire ];
    in {

      # ── Nix packages ────────────────────────────────────────────────────────
      packages = forAllSystems (pkgs: rec {
        # Offline dictionary data for the clipboard "define a word" panel, built
        # declaratively from pinned upstream sources: Webster's 1913 (English,
        # public domain) copied as-is, and the RAE dump (Spanish) parsed by
        # tools/rae-parse into `{word: {e,d}}` JSON. Installed to
        # $out/share/waverunner/, pointed at by $WAVERUNNER_DICT[_ES].
        # The card's talk-to-text: the speech engine (whisper.cpp) and its
        # model, pinned — so what is said is written out on the machine,
        # offline, and neither can be swept away by a clean-up (the first cut
        # borrowed a copy another package happened to depend on). The model is
        # the multilingual "base": 148 MB, a second or two for a sentence on a
        # laptop. Pointed at by $WAVERUNNER_WHISPER / $WAVERUNNER_WHISPER_MODEL
        # (`crates/daemon/src/card/voice.rs`).
        speech =
          let
            model = pkgs.fetchurl {
              # (Upstream's moving branch: the hash below is what pins it — a
              # changed file fails the build, it is never taken unseen.)
              url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin";
              hash = "sha256-YO1bw90U7qhWST0zQ0m0BXgt3K8AKNS130CINF+6Lv4=";
            };
          in
          pkgs.runCommand "waverunner-speech" { } ''
            mkdir -p $out/bin $out/share/waverunner
            ln -s ${pkgs.whisper-cpp}/bin/whisper-cli $out/bin/whisper-cli
            ln -s ${model} $out/share/waverunner/ggml-base.bin
          '';

        # The card's translator: an offline engine (translateLocally — marian
        # and the Bergamot models, the ones Firefox translates with) and the
        # language pairs it knows here, pinned: Spanish, French, German,
        # Portuguese and Italian, each to and from English (the small "tiny"
        # models, some 15–20 MB a pair). Spanish, French and German are
        # nixpkgs' own packages; Portuguese and Italian are not packaged
        # there, so they are put together here from the files Mozilla serves
        # to Firefox (the same kind of model: its three files, and the two
        # small ones translateLocally reads beside them). The engine finds
        # its models under share/translateLocally in the data dirs; it is run
        # with no window. Pointed at by $WAVERUNNER_TRANSLATE
        # (`crates/daemon/src/card/mod.rs`, `translate`).
        translate =
          let
            mozModel =
              {
                from,
                to,
                src,
                trg,
                model,
                lex,
                vocab,
              }:
              let
                pair = "${from}${to}";
                files = {
                  "model.${pair}.intgemm.alphas.bin" = pkgs.fetchurl model;
                  "lex.50.50.${pair}.s2t.bin" = pkgs.fetchurl lex;
                  "vocab.${pair}.spm" = pkgs.fetchurl vocab;
                };
                config = pkgs.writeText "config.intgemm8bitalpha.yml" ''
                  relative-paths: true
                  models:
                    - model.${pair}.intgemm.alphas.bin
                  vocabs:
                    - vocab.${pair}.spm
                    - vocab.${pair}.spm
                  shortlist:
                      - lex.50.50.${pair}.s2t.bin
                      - false
                  beam-size: 1
                  normalize: 1.0
                  word-penalty: 0
                  mini-batch: 64
                  maxi-batch: 1000
                  maxi-batch-sort: src
                  workspace: 2000
                  max-length-factor: 2.0
                  skip-cost: true
                  gemm-precision: int8shiftAlphaAll
                '';
                info = pkgs.writeText "model_info.json" (
                  builtins.toJSON {
                    modelName = "${src}-${trg} tiny";
                    shortName = "${from}-${to}-tiny";
                    type = "tiny";
                    inherit src trg;
                    srcTags = {
                      ${from} = src;
                    };
                    trgTag = to;
                    repository = "Mozilla";
                    version = 1;
                    API = 1;
                  }
                );
              in
              pkgs.runCommand "translatelocally-model-${from}-${to}-tiny" { } ''
                d=$out/share/translateLocally/models/${pair}.mozilla.tiny
                mkdir -p $d
                ${pkgs.lib.concatStringsSep "\n" (
                  pkgs.lib.mapAttrsToList (name: file: "ln -s ${file} $d/${name}") files
                )}
                ln -s ${config} $d/config.intgemm8bitalpha.yml
                ln -s ${info} $d/model_info.json
              '';
            models = (with pkgs.translatelocally-models; [
              es-en-tiny
              en-es-tiny
              fr-en-tiny
              en-fr-tiny
              de-en-tiny
              en-de-tiny
            ]) ++ [
              (mozModel {
                from = "pt";
                to = "en";
                src = "Portuguese";
                trg = "English";
                model = { url = "https://firefox-settings-attachments.cdn.mozilla.net/main-workspace/translations-models/dc4327ec-9ebc-4c12-8037-48cd30f3076d.bin"; sha256 = "b4a1fd101f59ca258bfb051a3a4e5762497b9f91d7e74f28b7a2d68dc075f8fa"; };
                lex = { url = "https://firefox-settings-attachments.cdn.mozilla.net/main-workspace/translations-models/013e0ebf-3d6b-4723-b83e-0e00ed29477f.bin"; sha256 = "702d9b09fbc020168998bb81afc927e74262f5a1455e9b443608a5d8efa4b52d"; };
                vocab = { url = "https://firefox-settings-attachments.cdn.mozilla.net/main-workspace/translations-models/75fa56af-540e-4a56-8a9f-1317ae7a9c61.spm"; sha256 = "2326a27577b5ffa0b822c55fe850986961728097521de35fcc186f7d6dce72d4"; };
              })
              (mozModel {
                from = "en";
                to = "pt";
                src = "English";
                trg = "Portuguese";
                model = { url = "https://firefox-settings-attachments.cdn.mozilla.net/main-workspace/translations-models/b268bf87-94b6-4893-9da1-c4e75284ace7.bin"; sha256 = "8fb05a27509bea3f67d2f59506485584d5cdbdcafa82b251576c27e91bd7011e"; };
                lex = { url = "https://firefox-settings-attachments.cdn.mozilla.net/main-workspace/translations-models/be3a2e24-b12e-4c0e-b07a-c7b0ba6ab421.bin"; sha256 = "dd0e64865277f0a1cf15a26a5b50e9e0570235afd8dab7c5e0249ad7057531d6"; };
                vocab = { url = "https://firefox-settings-attachments.cdn.mozilla.net/main-workspace/translations-models/745bff57-f929-41d9-8f0f-913513cfd334.spm"; sha256 = "2326a27577b5ffa0b822c55fe850986961728097521de35fcc186f7d6dce72d4"; };
              })
              (mozModel {
                from = "it";
                to = "en";
                src = "Italian";
                trg = "English";
                model = { url = "https://firefox-settings-attachments.cdn.mozilla.net/main-workspace/translations-models/1cf6a2f6-f430-4119-98d5-1ec0617beb0f.bin"; sha256 = "7dfdf189146d9353fdea264b9e4c8ac36441c770dc4353a8380b64e589dc035b"; };
                lex = { url = "https://firefox-settings-attachments.cdn.mozilla.net/main-workspace/translations-models/b38e032b-1b1a-4899-a0e5-80ef69a06814.bin"; sha256 = "e30ec549bd0da9ac42cccdcd3806d3be84d485f7fd329f90f6e40ee027e841d9"; };
                vocab = { url = "https://firefox-settings-attachments.cdn.mozilla.net/main-workspace/translations-models/fbc18b06-35c1-47fa-b2db-5b792366471b.spm"; sha256 = "603f3349657c3deb9736a0c567452d102a5a03c377dfdf1d32c428608f2cff1b"; };
              })
              (mozModel {
                from = "en";
                to = "it";
                src = "English";
                trg = "Italian";
                model = { url = "https://firefox-settings-attachments.cdn.mozilla.net/main-workspace/translations-models/efb14526-aff4-4826-8cf6-12e2bc44ac7d.bin"; sha256 = "3d7bbc4d7977e10b35f53faa79f5d5de8211f4f04baed9e7cd9dee1dcceda917"; };
                lex = { url = "https://firefox-settings-attachments.cdn.mozilla.net/main-workspace/translations-models/d683f5a2-7a75-444c-a581-22b3c3cfaecb.bin"; sha256 = "351ea80fb9f366f07533c7c4836248e72d9d4aa4eb7a05b5d74891a7abb4208c"; };
                vocab = { url = "https://firefox-settings-attachments.cdn.mozilla.net/main-workspace/translations-models/8d28b170-7607-4834-a548-2a1244f35576.spm"; sha256 = "603f3349657c3deb9736a0c567452d102a5a03c377dfdf1d32c428608f2cff1b"; };
              })
            ];
          in
          pkgs.runCommand "waverunner-translate" { nativeBuildInputs = [ pkgs.makeWrapper ]; } ''
            mkdir -p $out/bin $out/share/translateLocally/models
            for m in ${toString models}; do
              ln -s $m/share/translateLocally/models/* $out/share/translateLocally/models/
            done
            makeWrapper ${pkgs.translatelocally}/bin/translateLocally $out/bin/translateLocally \
              --prefix XDG_DATA_DIRS : $out/share \
              --set-default QT_QPA_PLATFORM offscreen
          '';

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
              --suffix LD_LIBRARY_PATH : /run/opengl-driver/lib \
              --set-default __EGL_VENDOR_LIBRARY_DIRS /run/opengl-driver/share/glvnd/egl_vendor.d \
              --prefix PATH : ${pkgs.lib.makeBinPath (runtimeTools pkgs)} \
              --set-default WAVERUNNER_DICT ${dictionaries}/share/waverunner/dictionary.json \
              --set-default WAVERUNNER_DICT_ES ${dictionaries}/share/waverunner/dictionary-es.json \
              --set-default WAVERUNNER_WHISPER ${speech}/bin/whisper-cli \
              --set-default WAVERUNNER_WHISPER_MODEL ${speech}/share/waverunner/ggml-base.bin \
              --set-default WAVERUNNER_TRANSLATE ${translate}/bin/translateLocally
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
            nativeBuildInputs = [ pkgs.nix pkgs.jq pkgs.nix-index waverunner-daemon ];
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
          # Desktop stems + in-package icon paths from the offline file
          # database (nix-locate wants a dir containing `files`) —
          # without them multi-desktop suites can't resolve gui.
          mkdir db
          ln -s ${
            nix-index-database.packages.${pkgs.stdenv.hostPlatform.system}.nix-index-database
          } db/files
          export WAVERUNNER_NIX_INDEX_DB=$PWD/db
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
                # NEVER give up on the shell (F8). systemd's default limit is
                # 5 starts in 10s, so with RestartSec=1s a daemon that failed
                # on something environmental — a GPU not ready yet, a driver
                # still settling — burned through its budget in about five
                # seconds and was left permanently failed: no dock, no bar,
                # no explanation, and no way back without a terminal. A shell
                # that keeps trying is strictly better than one that dies,
                # and the daemon now also survives renderer failure on its
                # own rather than exiting.
                StartLimitIntervalSec = 0;
              };
              Service = {
                ExecStart = "${cfg.package}/bin/waverunner";
                Restart   = "on-failure";
                # Slower than 1s: a retry storm helps nothing, and the cause
                # usually needs a moment to clear.
                RestartSec = "2s";
              } // (let
                env =
                  lib.optional (cfg.packageIndex != null)
                    "WAVERUNNER_PKG_INDEX=${cfg.packageIndex}/share/waverunner/nixpkgs-index.tsv";
              in lib.optionalAttrs (env != [ ]) { Environment = env; });
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
