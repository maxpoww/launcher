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
#   verify-ui.sh [--build] [--restart]
#                [--reveal dock|open|clip|clip-detail|clip-search|notif|sunset|none]
#                [--query TEXT] [--geom "X,Y WxH"] [--out PATH]
#
# --query is the text `--reveal clip-search` types into the clipboard box's
# type-to-search field (empty = the bare field, nothing typed).
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
    --query)    REVEAL_ARG="${2:-}"; shift ;;
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
  # Give the layer surface time to map. Poll instead of a flat sleep — GPU/
  # shader pipeline setup (wgpu adapter probe, WGSL compile) has been seen to
  # take past 1.5s on this machine, which made a live daemon look dead.
  up=0
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if pgrep -f "$DAEMON_PAT" >/dev/null; then up=1; break; fi
    sleep 0.3
  done
  if [ "$up" != 1 ]; then
    echo "!! daemon did not come back up — check $DEV_LAUNCHER" >&2
    exit 1
  fi
  # The process existing isn't the same as the surface being mapped yet —
  # keep the old fixed settle after the poll confirms it's alive.
  sleep 1.5
fi

case "$reveal" in
  dock)        "$CTL" show 2>/dev/null || true; sleep 0.5 ;;
  open)        "$CTL" expand 2>/dev/null || true; sleep 0.6 ;;
  clip)        "$CTL" debug-clip 2>/dev/null || true; sleep 0.8 ;;
  clip-detail) "$CTL" debug-clip-detail 2>/dev/null || true; sleep 0.9 ;;
  clip-search) "$CTL" debug-clip-search "${REVEAL_ARG:-}" 2>/dev/null || true; sleep 0.9 ;;
  emoji)       "$CTL" debug-emoji "${REVEAL_ARG:-}" 2>/dev/null || true; sleep 0.9 ;;
  notif)       "$CTL" debug-notif 2>/dev/null || true; sleep 0.8 ;;
  sunset)      "$CTL" debug-sunset 2>/dev/null || true; sleep 0.8 ;;
  none) ;;
  *) echo "verify-ui: --reveal must be dock|open|clip|clip-detail|clip-search|notif|sunset|none" >&2; exit 2 ;;
esac

echo ">> capturing screenshot…" >&2
if [ -n "$geom" ]; then
  grim -g "$geom" "$out"
else
  grim "$out"
fi

echo "$out"
