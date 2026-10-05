# threadcpu.sh <workload>: per-thread CPU of the dock (and its children) over one workload, and a whole-process profile
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
PERF=/nix/var/nix/gcroots/lab/perf/bin/perf; CTL=/nix/var/nix/gcroots/lab/ctl/bin/waverunner-ctl
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"
C="$E $CTL"; DP=$(pgrep -f "bin/waverunner$" | head -1); hz=$(getconf CLK_TCK)
ptr() { $E hyprctl eval "hl.dispatch(hl.dsp.cursor.move({ x = $1, y = $2 }))" >/dev/null 2>&1; }
read LW LH <<< "$($E hyprctl -j monitors | jq -r '.[0] | "\((.width/.scale)|floor) \((.height/.scale)|floor)"')"; CX=$((LW/2))
wl_stage() { $E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c 'sleep 40' >/dev/null 2>&1 </dev/null &"; sleep 2; $E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c 'sleep 40' >/dev/null 2>&1 </dev/null &"; sleep 3
  for i in 1 2 3; do $C stage-toggle; sleep 3; $C stage-pick 2; sleep 2; $C stage-toggle; sleep 2.5; done; pkill -x foot; sleep 2; }
wl_panel() { for i in 1 2 3 4; do $C display show scale; sleep 2.2; $C hide; sleep 1.3; done; }
wl_launcher() { for i in 1 2 3 4 5; do $C show; sleep 0.8; $C expand; sleep 1.6; $C collapse; sleep 0.9; $C hide; sleep 1.0; done; }
wl_typing() { $C show; sleep 0.6; $C expand; sleep 1.4; for q in l li lib libr libre libreo libreof libreoff libreoffi libreoffic libreoffice; do $C debug-query $q; sleep 0.3; done; sleep 0.5; $C collapse; sleep 0.9; $C hide; sleep 1.0; }
wl_boxes() { for i in 1 2 3 4; do $C debug-notif; sleep 0.9; ptr $((LW-100)) 120; sleep 0.5; ptr 1000 600; sleep 1.3; $C debug-stats; sleep 0.9; ptr 120 80; sleep 0.5; ptr 1000 600; sleep 1.3; $C debug-clip; sleep 0.9; ptr 250 120; sleep 0.5; ptr 1000 600; sleep 1.3; done; }
snap() { for t in /proc/$DP/task/*; do echo "$(basename $t) $(cat $t/comm 2>/dev/null | tr ' ' '_') $(awk '{print $14+$15}' $t/stat 2>/dev/null)"; done; echo "children - $(awk '{print $16+$17}' /proc/$DP/stat)"; }
ptr 1000 600; $C hide; sleep 1
snap > /tmp/tc.0
$PERF record -q -g -p $DP -o /tmp/tp.data sleep 9999 >/dev/null 2>&1 & PP=$!; sleep 0.3
wl_$1
pkill -P $PP -x sleep; wait $PP 2>/dev/null
snap > /tmp/tc.1
echo "== $1: CPU seconds per thread of the dock"
join -a2 <(sort /tmp/tc.0) <(sort /tmp/tc.1) | awk -v hz=$hz '{ if (NF==5) { d=($5-$3)/hz; n=$2 } else { d=$3/hz; n=$2 } if (d>=0.02) printf "  %-18s %.2f s\n", n, d }' | sort -k2 -nr | awk '{a[$1]+=$2} END{for(k in a) printf "  %-18s %.2f s\n", k, a[k]}' | sort -k2 -nr
echo "== by thread and symbol (self, >= 1.2 %)"
$PERF report -i /tmp/tp.data --no-children --stdio -g none --sort comm,dso,symbol --percent-limit 1.2 2>/dev/null | grep -v "^#" | grep "%" | sed "s/\[kernel.kallsyms\]/kernel/; s/\.waverunner-wrapped/dock/" | cut -c1-150 | head -45
