# work.sh <tag> [workloads...]: scripted shell workloads with CPU / GPU / frame counters. Run as root on the laptop.
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
PERF=/nix/var/nix/gcroots/lab/perf/bin/perf; CTLBIN=/nix/var/nix/gcroots/lab/ctl/bin/waverunner-ctl
TAG=$1; shift; WL="${@:-idle boxes launcher fast panel hover typing notifs stage}"
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"
C="$E $CTLBIN"
ptr() { $E hyprctl eval "hl.dispatch(hl.dsp.cursor.move({ x = $1, y = $2 }))" >/dev/null 2>&1; }
GRIM=$(ls -d /nix/store/*-grim-*/bin/grim | head -1)
DP=$(pgrep -f "bin/waverunner$" | head -1); HP=$(pgrep -x .Hyprland-wrapp | head -1); hz=$(getconf CLK_TCK)
cpu() { awk '{print $14+$15}' /proc/$1/stat; }
# GPU render-engine time (ns) of a process: one entry per DRM client
gpu() { for f in /proc/$1/fdinfo/*; do awk '/drm-client-id/{id=$2} /drm-engine-render/{ns=$2} END{if(id!="") print id, ns}' $f 2>/dev/null; done | sort -u | awk '{s+=$2} END{printf "%.0f", s}'; }
# logical screen size (pointer coordinates)
read LW LH <<< "$($E hyprctl -j monitors | jq -r '.[0] | "\((.width/.scale)|floor) \((.height/.scale)|floor)"')"
CX=$((LW/2)); CY=$((LH*55/100))
park() { ptr $CX $CY; }

wl_boxes() { for i in 1 2 3 4; do
    $C debug-notif; sleep 0.9; ptr $((LW-100)) 120; sleep 0.5; park; sleep 1.3
    $C debug-stats; sleep 0.9; ptr 120 80; sleep 0.5; park; sleep 1.3
    $C debug-clip;  sleep 0.9; ptr 250 120; sleep 0.5; park; sleep 1.3
  done; }
wl_launcher() { for i in 1 2 3 4 5; do $C show; sleep 0.8; $C expand; sleep 1.6; $C collapse; sleep 0.9; $C hide; sleep 1.0; done; }
wl_fast() { for i in 1 2 3 4 5; do $C fast-launch; sleep 1.3; $C fast-launch; sleep 1.0; done; }
wl_panel() { for i in 1 2 3 4; do $C display show scale; sleep 2.2; $C hide; sleep 1.3; done; }
wl_hover() { $C show; sleep 1; for s in 1 2 3; do x=$((CX-160)); while [ $x -le $((CX+160)) ]; do ptr $x $((LH-40)); x=$((x+8)); done; done; park; sleep 0.5; $C hide; sleep 1; }
wl_typing() { $C show; sleep 0.6; $C expand; sleep 1.4; for q in l li lib libr libre libreo libreof libreoff libreoffi libreoffic libreoffice; do $C debug-query $q; sleep 0.3; done; sleep 0.5; $C collapse; sleep 0.9; $C hide; sleep 1.0; }
wl_notifs() { for i in 1 2 3 4 5; do $E notify-send -a "Perf" "Workload $i" "unify round 3"; sleep 1.6; done; sleep 1; }
wl_stage() { $E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c 'sleep 40' >/dev/null 2>&1 </dev/null &"; sleep 2; $E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c 'sleep 40' >/dev/null 2>&1 </dev/null &"; sleep 3
  for i in 1 2 3; do $C stage-toggle; sleep 3; $C stage-pick 2; sleep 2; $C stage-toggle; sleep 2.5; done; pkill -x foot; sleep 2; }
wl_idle() { sleep 30; }

park; $C hide; sleep 2
printf "%-9s %6s %8s %8s %8s %8s %7s %7s %7s %5s  %s\n" workload wall_s dock_cpu hypr_cpu dock_gpu hypr_gpu gpu_busy dock_Gi hypr_Gi GHz counters
for w in $WL; do
  $C debug-perf; sleep 0.3
  T0=$(date "+%F %T"); t0=$(date +%s.%N); d0=$(cpu $DP); h0=$(cpu $HP); dg0=$(gpu $DP); hg0=$(gpu $HP)
  $PERF stat -a -e i915/rcs0-busy/ -x, -o /tmp/gpu.$w sleep 9999 & PP=$!
  $PERF stat -p $HP -e instructions,cycles -x, -o /tmp/hi.$w sleep 9999 2>/dev/null & PH=$!
  $PERF stat -p $DP -e instructions,cycles -x, -o /tmp/di.$w sleep 9999 2>/dev/null & PD=$!
  sleep 0.2
  wl_$w
  pkill -P $PP -x sleep; pkill -P $PH -x sleep; pkill -P $PD -x sleep; wait $PP $PH $PD 2>/dev/null
  t1=$(date +%s.%N); d1=$(cpu $DP); h1=$(cpu $HP); dg1=$(gpu $DP); hg1=$(gpu $HP)
  $C debug-perf; sleep 0.4
  $E $GRIM /tmp/wl-$TAG-$w.png 2>/dev/null
  line=$(runuser -u max -- env XDG_RUNTIME_DIR=/run/user/1000 journalctl --user -u waverunner --since "$T0" -o cat | sed "s/\x1b\[[0-9;]*m//g" | grep "perf: " | tail -1 | sed 's/.*perf: //')
  busy=$(grep -m1 rcs0-busy /tmp/gpu.$w | cut -d, -f1)
  hi=$(grep -m1 instructions /tmp/hi.$w | cut -d, -f1); hc=$(grep -m1 cycles /tmp/hi.$w | cut -d, -f1)
  di=$(grep -m1 instructions /tmp/di.$w | cut -d, -f1); dc=$(grep -m1 cycles /tmp/di.$w | cut -d, -f1)
  awk -v w=$w -v t0=$t0 -v t1=$t1 -v d0=$d0 -v d1=$d1 -v h0=$h0 -v h1=$h1 -v hz=$hz -v busy="$busy" -v dg0=$dg0 -v dg1=$dg1 -v hg0=$hg0 -v hg1=$hg1 -v line="$line" -v hi="$hi" -v hc="$hc" -v di="$di" -v dc="$dc" 'BEGIN{wall=t1-t0; hs=(h1-h0)/hz; printf "%-9s %6.1f %7.2fs %7.2fs %7.2fs %7.2fs %6.1f%% %7.2f %7.2f %5.2f  %s\n", w, wall, (d1-d0)/hz, hs, (dg1-dg0)/1e9, (hg1-hg0)/1e9, 100*busy/1e9/wall, di/1e9, hi/1e9, (hs>0? hc/1e9/hs : 0), line}'
done
[ "$(pgrep -f "bin/waverunner$" | head -1)" = "$DP" ] || echo "DOCK RESTARTED during the run"
