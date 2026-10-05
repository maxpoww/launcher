# video.sh <tag> [seconds]: a windowed video (a 30 fps test pattern in mpv) plays while the shell rests. What it costs
# Hyprland and the GPU, and how many captures the dock takes meanwhile.
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
PERF=/nix/var/nix/gcroots/lab/perf/bin/perf; CTLBIN=/nix/var/nix/gcroots/lab/ctl/bin/waverunner-ctl; N=${2:-12}
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"
C="$E $CTLBIN"; HP=$(pgrep -x .Hyprland-wrapp | head -1); DP=$(pgrep -f "bin/waverunner$" | head -1); hz=$(getconf CLK_TCK)
cpu() { awk '{print $14+$15}' /proc/$1/stat; }
gpu() { for f in /proc/$1/fdinfo/*; do awk '/drm-client-id/{id=$2} /drm-engine-render/{ns=$2} END{if(id!="") print id, ns}' $f 2>/dev/null; done | sort -u | awk '{s+=$2} END{printf "%.0f", s}'; }
$E hyprctl eval 'hl.dispatch(hl.dsp.cursor.move({ x = 1000, y = 600 }))' >/dev/null; $C hide
$E sh -c "timeout -s KILL $((N+9)) mpv --no-config --really-quiet --no-audio --load-scripts=no --force-window=yes --geometry=640x360 'av://lavfi:testsrc=size=640x360:rate=30' >/dev/null 2>&1 </dev/null &"
sleep 5
$C debug-perf; sleep 0.3; T0=$(date "+%F %T")
h0=$(cpu $HP); g0=$(gpu $HP); d0=$(cpu $DP); t0=$(date +%s.%N)
$PERF stat -a -e i915/rcs0-busy/ -x, -o /tmp/gpu.vd sleep $N 2>/dev/null
h1=$(cpu $HP); g1=$(gpu $HP); d1=$(cpu $DP); t1=$(date +%s.%N)
$C debug-perf; sleep 0.4
line=$(runuser -u max -- env XDG_RUNTIME_DIR=/run/user/1000 journalctl --user -u waverunner --since "$T0" -o cat | sed "s/\x1b\[[0-9;]*m//g" | grep "perf: " | tail -1 | sed 's/.*perf: //')
cap=$(echo "$line" | grep -o "captures [0-9]*"); fr=$(echo "$line" | grep -o "render [0-9]*"); echo "   $line" | cut -c1-420
awk -v l="$1" -v h0=$h0 -v h1=$h1 -v g0=$g0 -v g1=$g1 -v d0=$d0 -v d1=$d1 -v hz=$hz -v t0=$t0 -v t1=$t1 -v busy="$(grep -m1 rcs0 /tmp/gpu.vd | cut -d, -f1)" -v cap="$cap" -v fr="$fr" 'BEGIN{w=t1-t0; printf "%-22s %4.1f s of video: hyprland cpu %.2fs gpu %.2fs | dock cpu %.2fs | GPU busy %.1f%% | %s, %s\n", l, w, (h1-h0)/hz, (g1-g0)/1e9, (d1-d0)/hz, 100*busy/1e9/w, cap, fr}'
$E hyprctl -j clients | jq -r '.[] | "   window \(.class) \(.size[0])x\(.size[1]) floating=\(.floating)"' | head -2
pkill -x mpv; sleep 2
