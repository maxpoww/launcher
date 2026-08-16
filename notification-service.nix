# notification-service.nix
#
# Makes OPTIONS the system's primary desktop notification daemon: a systemd
# user service that runs `options-notify`, which owns
# `org.freedesktop.Notifications` on the session bus.
#
# A D-Bus well-known name has a single owner, so NO other notification daemon
# (mako, dunst, …) may be installed/running — otherwise this service will fail
# to claim the name and restart-loop.
#
# Usage in your NixOS configuration (flake-based, with the waverunner flake as
# an input named e.g. `waverunner`):
#
#   imports = [ ./notification-service.nix ];
#   services.options-notify = {
#     enable  = true;
#     package = inputs.waverunner.packages.${pkgs.stdenv.hostPlatform.system}.options-notify;
#     enableKdeConnect = true;   # cross-device bridging (WhatsApp via KDE Connect)
#   };

{ config, lib, pkgs, ... }:

let
  cfg = config.services.options-notify;
in
{
  options.services.options-notify = {
    enable = lib.mkEnableOption
      "OPTIONS desktop notification daemon (org.freedesktop.Notifications server)";

    package = lib.mkOption {
      type = lib.types.package;
      description = "The options-notify package to run (from the waverunner flake).";
      example = lib.literalExpression
        "inputs.waverunner.packages.\${pkgs.stdenv.hostPlatform.system}.options-notify";
    };

    enableKdeConnect = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Opt-in phone-notification bridge — OPTIONS does NOT depend on this; it's
        purely for users who already use KDE Connect. When enabled, the KDE
        Connect daemon runs and its "receive notifications" plugin forwards a
        paired phone's notifications onto org.freedesktop.Notifications, where the
        OPTIONS daemon picks them up like any other notification (no extra wiring;
        phone-app icon + text come through as-is). It covers apps that can't
        notify from a closed desktop (WhatsApp, Messenger, …) by using the phone
        as the always-on source.

        One-time setup after enabling: pair the phone in the KDE Connect app and
        turn on its notification-sync plugin. Left off by default — a Golem
        desktop is fully functional without it.
      '';
    };

    logLevel = lib.mkOption {
      type = lib.types.str;
      default = "info";
      description = "RUST_LOG level for the daemon (e.g. \"debug\").";
    };
  };

  config = lib.mkIf cfg.enable {
    # Cross-device bridging (optional): the KDE Connect NixOS module already
    # provides the daemon, its D-Bus service files, and firewall pinholes — no
    # need to add the package to services.dbus.packages (and the attribute name
    # varies across nixpkgs: pkgs.kdeconnect vs pkgs.kdePackages.kdeconnect-kde).
    programs.kdeconnect.enable = lib.mkIf cfg.enableKdeConnect true;

    # The notification backend, tied to the graphical session. Type = "dbus"
    # with BusName means systemd considers the unit *started* only once the name
    # is actually acquired on the bus — clean readiness + ordering for anything
    # that depends on the notification service being up.
    systemd.user.services.options-notify = {
      description = "OPTIONS Desktop Notification Engine & D-Bus Server";
      documentation = [ "https://github.com/maxpoww/launcher" ];
      wantedBy = [ "graphical-session.target" ];
      partOf = [ "graphical-session.target" ];
      after = [ "graphical-session.target" ];
      serviceConfig = {
        Type = "dbus";
        BusName = "org.freedesktop.Notifications";
        ExecStart = "${cfg.package}/bin/options-notify";
        # Always keep the notification server up (a clean `systemctl stop` is
        # still honoured); the daemon exits non-zero if the bus drops so we
        # reclaim the name after a session-bus restart.
        Restart = "always";
        RestartSec = "2s";
        Environment = [ "RUST_LOG=options_notify=${cfg.logLevel}" ];
        # Hardening: it needs only the session bus and network (KDE Connect
        # bridge later); keep it otherwise confined.
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = "read-only";
        # NOT PrivateTmp: notifying apps (Chrome/Chromium web notifications)
        # write their image assets — the site logo and per-message avatar — to
        # scoped-temp dirs under the host /tmp and delete them moments later. The
        # daemon must read those files at Notify time to capture the image, so it
        # needs the shared /tmp, not a private one.
        PrivateTmp = false;
      };
    };
  };
}
