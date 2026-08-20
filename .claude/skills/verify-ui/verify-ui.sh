#!/usr/bin/env bash
# verify-ui.sh — close the visual feedback loop for waverunner.
#
# Builds the daemon, (optionally) restarts the running instance so the new
# binary is live, (optionally) reveals the dock/launcher, then grabs a
# screenshot with grim and prints its path for Claude to Read.
#
# Safe by design: it only ever touches the waverunner *dev* binary and the
# screenshot file under /tmp. It never runs nixos-rebuild, sudo, or anything
# that can affect the system or the boot.
#
# Usage:
#   verify-ui.sh [--build] [--restart] [--reveal dock|open|none]
#                [--geom "X,Y WxH"] [--out PATH]
#
# Defaults: --build --restart --reveal none  (screenshot the whole screen)
set -euo pipefail

REPO="/home/max/launcher"
DEV_LAUNCHER="$REPO/waverunner-dev"
CTL="$REPO/target/debug/waverunner-ctl"
SHOT_DIR="/tmp/waverunner-verify"
# Regex (pgrep/pkill -f) matching the daemon by its cmdline whether argv[0] is
# the absolute or relative path, and WITHOUT matching waverunner-ctl (shares the
# prefix): the path ends right after "waverunner", at end-of-arg or a space.
DAEMON_PAT='target/debug/waverunner($| )'

do_build=1
do_restart=1
reveal="none"
geom=""
out=""

while [ $# -gt 0 ]; do
  case "$1" in
    --build)    do_build=1 ;;
    --no-build) do_build=0 ;;
    --restart)  do_restart=1 ;;
    --no-restart) do_restart=0 ;;
    --reveal)   reveal="${2:-none}"; shift ;;
    --geom)     geom="${2:-}"; shift ;;
    --out)      out="${2:-}"; shift ;;
    *) echo "verify-ui: unknown arg: $1" >&2; exit 2 ;;
  esac
  shift
done

mkdir -p "$SHOT_DIR"
[ -n "$out" ] || out="$SHOT_DIR/shot-$(date +%H%M%S).png"

if [ "$do_build" = 1 ]; then
  echo ">> building (nix develop -c cargo build)…" >&2
  if ! (cd "$REPO" && nix develop -c cargo build 2>&1 | tail -3 >&2); then
    echo "!! build FAILED — leaving the running daemon untouched." >&2
    exit 1
  fi
fi

if [ "$do_restart" = 1 ]; then
  echo ">> restarting daemon…" >&2
  # Kill only the daemon (never waverunner-ctl / this script).
  pkill -f "$DAEMON_PAT" 2>/dev/null || true
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    pgrep -f "$DAEMON_PAT" >/dev/null || break
    sleep 0.15
  done
  # Relaunch detached, exactly as Hyprland does (waverunner-dev sets LD paths).
  # Keep the daemon's stdout/stderr in a log so runtime errors — notably wgpu
  # validating WGSL shaders at pipeline creation — are inspectable after a crash.
  setsid "$DEV_LAUNCHER" >"$SHOT_DIR/daemon.log" 2>&1 < /dev/null &
  # Give the layer surface time to map.
  sleep 1.5
  if ! pgrep -f "$DAEMON_PAT" >/dev/null; then
    echo "!! daemon did not come back up — check $DEV_LAUNCHER" >&2
    exit 1
  fi
fi

case "$reveal" in
  dock)        "$CTL" show 2>/dev/null || true; sleep 0.5 ;;
  open)        "$CTL" expand 2>/dev/null || true; sleep 0.6 ;;
  clip)        "$CTL" debug-clip 2>/dev/null || true; sleep 0.8 ;;
  clip-detail) "$CTL" debug-clip-detail 2>/dev/null || true; sleep 0.9 ;;
  notif)       "$CTL" debug-notif 2>/dev/null || true; sleep 0.8 ;;
  none) ;;
  *) echo "verify-ui: --reveal must be dock|open|clip|clip-detail|notif|none" >&2; exit 2 ;;
esac

echo ">> capturing screenshot…" >&2
if [ -n "$geom" ]; then
  grim -g "$geom" "$out"
else
  grim "$out"
fi

echo "$out"
