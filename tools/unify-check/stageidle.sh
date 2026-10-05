# stageidle.sh [seconds] [sound]: what STAGE mode costs while nothing happens (two terminals, the deck up).
# With "sound": a silent-but-running audio stream plays in one of the terminals, to see the speaker badge.
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
PERF=/nix/var/nix/gcroots/lab/perf/bin/perf; CTL=/nix/var/nix/gcroots/lab/ctl/bin/waverunner-ctl
N=${1:-30}
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"
C="$E $CTL"; DP=$(pgrep -f "bin/waverunner$" | head -1); hz=$(getconf CLK_TCK)
$E hyprctl eval "hl.dispatch(hl.dsp.cursor.move({ x = 1000, y = 500 }))" >/dev/null 2>&1; $C hide
$E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c 'sleep $((N+40))' >/dev/null 2>&1 </dev/null &"; sleep 2
$E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c 'sleep $((N+40))' >/dev/null 2>&1 </dev/null &"; sleep 3
$C stage-toggle; sleep 6
ticks() { awk '{print $14+$15}' /proc/$1/stat 2>/dev/null || echo 0; }
kids() { awk '{print $16+$17}' /proc/$1/stat; }
pwsum() { s=0; for p in $(pgrep -x pipewire; pgrep -x wireplumber; pgrep -x pipewire-pulse); do s=$((s+$(ticks $p))); done; echo $s; }
allcpu() { awk '/^cpu /{print $2+$3+$4+$7+$8}' /proc/stat; }
d0=$(ticks $DP); k0=$(kids $DP); p0=$(pwsum); a0=$(allcpu)
$PERF stat -a -e sched:sched_process_exec -x, -o /tmp/si.exec sleep $N 2>/dev/null
d1=$(ticks $DP); k1=$(kids $DP); p1=$(pwsum); a1=$(allcpu)
ex=$(grep -m1 sched_process_exec /tmp/si.exec | cut -d, -f1)
nc=$(nproc)
awk -v n=$N -v hz=$hz -v d=$((d1-d0)) -v k=$((k1-k0)) -v p=$((p1-p0)) -v a=$((a1-a0)) -v ex=$ex -v nc=$nc 'BEGIN{
  printf "stage at rest, %d s: dock %.2f s CPU + its helpers %.2f s + PipeWire %.2f s = %.1f %% of one core; whole machine %.1f %% of %d cores; %d processes started (%.1f/s)\n",
    n, d/hz, k/hz, p/hz, 100*(d+k+p)/hz/n, 100*a/hz/n/nc, nc, ex, ex/n }'
$C stage-toggle; sleep 2.5; pkill -x foot; sleep 1.5
