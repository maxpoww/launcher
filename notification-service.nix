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
        Also run the KDE Connect daemon (headless) so cross-device
        notifications (e.g. WhatsApp on a paired phone) can be bridged into
        OPTIONS.
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
        PrivateTmp = true;
      };
    };
  };
}
