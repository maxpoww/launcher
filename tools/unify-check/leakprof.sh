# leakprof.sh <recordings>: make N short screen recordings, then profile the compositor (sampling only)
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
PERF=/nix/var/nix/gcroots/lab/perf/bin/perf; WF=/nix/var/nix/gcroots/lab/wf-recorder/bin/wf-recorder
HP=$(pgrep -x .Hyprland-wrapp | head -1)
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H"
C="$E /nix/var/nix/gcroots/lab/ctl/bin/waverunner-ctl"; hz=$(getconf CLK_TCK)
cpu() { awk '{print $14+$15}' /proc/$HP/stat; }
act() { for i in $(seq 1 $1); do $C debug-stats; sleep 0.8; $C debug-stats; sleep 0.8; done; }
measure() { c0=$(cpu); $PERF stat -p $HP -e instructions -x, -o /tmp/lp.csv sleep 9999 2>/dev/null & PP=$!; sleep 0.2; act 5; pkill -P $PP -x sleep; wait $PP 2>/dev/null; c1=$(cpu)
  echo "$1: hyprland cpu $(( (c1-c0)*1000/hz )) ms, $(awk -F, '/instructions/{printf "%.2f", $1/1e9}' /tmp/lp.csv) G instructions for 5 open/close cycles"; }
measure "before"
for r in $(seq 1 ${1:-1}); do
  $E $WF -y -f /tmp/leak.mkv -c libx264rgb -p crf=0 -p preset=ultrafast >/dev/null 2>&1 & sleep 1.5; act 6; pkill -INT -x wf-recorder; sleep 2.5
  measure "after recording $r"
done
($PERF record -q -p $HP -o /tmp/lp.data sleep 9 >/dev/null 2>&1 &); sleep 0.3; act 5; sleep 1.5
$PERF report -i /tmp/lp.data --no-children --percent-limit 2 --stdio 2>/dev/null | grep -v "^#" | grep "%" | head -12
