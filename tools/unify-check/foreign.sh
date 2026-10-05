# foreign.sh <tag>: what do OUR (mostly transparent) blurred layers cost while ANOTHER client draws?
# A terminal scrolls text for 10 s; Hyprland's GPU time with the shell's layers blurred, and with their blur rule off.
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
PERF=/nix/var/nix/gcroots/lab/perf/bin/perf
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H"
HP=$(pgrep -x .Hyprland-wrapp | head -1); hz=$(getconf CLK_TCK)
cpu() { awk '{print $14+$15}' /proc/$1/stat; }
gpu() { for f in /proc/$1/fdinfo/*; do awk '/drm-client-id/{id=$2} /drm-engine-render/{ns=$2} END{if(id!="") print id, ns}' $f 2>/dev/null; done | sort -u | awk '{s+=$2} END{printf "%.0f", s}'; }
$E hyprctl eval 'hl.dispatch(hl.dsp.cursor.move({ x = 1000, y = 600 }))' >/dev/null
run() {
  $E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c 'sleep 2; timeout 10 sh -c \"while :; do echo the quick brown fox jumps over the lazy dog 0123456789 the quick brown fox; done\"; sleep 2' >/dev/null 2>&1 </dev/null &"
  sleep 3.5
  h0=$(cpu $HP); g0=$(gpu $HP); t0=$(date +%s.%N)
  $PERF stat -a -e i915/rcs0-busy/ -x, -o /tmp/gpu.fg sleep 9999 2>/dev/null & PP=$!
  sleep 8
  pkill -P $PP -x sleep; wait $PP 2>/dev/null
  h1=$(cpu $HP); g1=$(gpu $HP); t1=$(date +%s.%N)
  awk -v l="$1" -v h0=$h0 -v h1=$h1 -v g0=$g0 -v g1=$g1 -v hz=$hz -v t0=$t0 -v t1=$t1 -v busy="$(grep -m1 rcs0 /tmp/gpu.fg | cut -d, -f1)" 'BEGIN{printf "%-34s hyprland cpu %.2fs gpu %.2fs, GPU busy %.1f%% (8 s of a terminal scrolling)\n", l, (h1-h0)/hz, (g1-g0)/1e9, 100*busy/1e9/(t1-t0)}'
  sleep 3; pkill -x foot; sleep 1.5
}
MODE=${2:-all}
run "$1: shell layers blurred:"
if [ "$MODE" = all ]; then
$E hyprctl eval 'hl.layer_rule({ match = { namespace = "waverunner" }, blur = false })' >/dev/null
$E hyprctl eval 'hl.layer_rule({ match = { namespace = "waverunner-options" }, blur = false })' >/dev/null
run "$1: their blur rule off:"
$E hyprctl reload >/dev/null 2>&1; sleep 2
fi
