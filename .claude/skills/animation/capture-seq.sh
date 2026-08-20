#!/usr/bin/env bash
# capture-seq.sh — capture an OPTIONS animation as a strip of frames so motion
# (not just a settled still) can be judged. Restarts the daemon to a clean
# closed state, triggers the animation, and bursts `grim` shots across it.
#
# Usage:
#   capture-seq.sh <what> [geom] [frames]
#     what   = clip-open | clip-detail | notif-open | dock | launcher
#     geom   = grim region "X,Y WxH" (default: left-edge box "0,0 420x520")
#     frames = number of shots to burst (default 8)
#
# Frames land in /tmp/waverunner-verify/seq/frame-NN.png; the paths are printed.
# Read the first / a middle / the last frame to see how the motion progresses.
set -euo pipefail

REPO="/home/max/launcher"
CTL="$REPO/target/debug/waverunner-ctl"
VERIFY="$REPO/.claude/skills/verify-ui/verify-ui.sh"
SEQ="/tmp/waverunner-verify/seq"

what="${1:?usage: capture-seq.sh <clip-open|clip-detail|notif-open|dock|launcher> [geom] [frames]}"
geom="${2:-0,0 420x520}"
frames="${3:-8}"

rm -rf "$SEQ"; mkdir -p "$SEQ"

# Build once and restart to a clean, fully-closed state (no reveal).
"$VERIFY" --no-restart >/dev/null 2>&1 || true   # build only, keep daemon
"$VERIFY" --no-build --reveal none >/dev/null 2>&1 # restart clean

case "$what" in
  clip-open)    pre=""            ; trigger="debug-clip" ;;
  # Open the box first (settle), then isolate the DETAIL open animation —
  # that's how a user meets it (right-click a row in an already-open box).
  clip-detail)  pre="debug-clip"  ; trigger="debug-clip-detail" ;;
  notif-open)   pre=""            ; trigger="debug-notif" ;;
  dock)         pre=""            ; trigger="show" ;;
  launcher)     pre="show"        ; trigger="expand" ;;
  *) echo "capture-seq: unknown target '$what'" >&2; exit 2 ;;
esac

if [ -n "$pre" ]; then "$CTL" "$pre" >/dev/null 2>&1 || true; sleep 0.7; fi

# Fire the animation and immediately burst-capture across it. ~45ms/frame ≈ a
# ~0.36s window over 8 frames, matching the OPTIONS open/close durations.
"$CTL" "$trigger" >/dev/null 2>&1 || true &
for i in $(seq 0 $((frames-1))); do
  n=$(printf '%02d' "$i")
  grim -g "$geom" "$SEQ/frame-$n.png" 2>/dev/null || true
  sleep 0.045
done
wait 2>/dev/null || true

echo "frames in $SEQ:"
ls "$SEQ"/frame-*.png
