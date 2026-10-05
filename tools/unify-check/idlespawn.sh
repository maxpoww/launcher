# idlespawn.sh [seconds]: at rest, how many processes start, and what the dock, its helpers and PipeWire cost
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
PERF=/nix/var/nix/gcroots/lab/perf/bin/perf; N=${1:-60}
DP=$(pgrep -f "bin/waverunner$" | head -1); hz=$(getconf CLK_TCK)
ticks() { awk '{print $14+$15}' /proc/$1/stat 2>/dev/null || echo 0; }
kids() { awk '{print $16+$17}' /proc/$1/stat; }
pwsum() { s=0; for p in $(pgrep -x pipewire; pgrep -x wireplumber; pgrep -x pipewire-pulse); do s=$((s+$(ticks $p))); done; echo $s; }
d0=$(ticks $DP); k0=$(kids $DP); p0=$(pwsum)
$PERF record -q -a -e sched:sched_process_exec -o /tmp/ie.data sleep $N >/dev/null 2>&1
d1=$(ticks $DP); k1=$(kids $DP); p1=$(pwsum)
echo "at rest, $N s: dock $(awk -v a=$((d1-d0)) -v hz=$hz 'BEGIN{printf "%.2f", a/hz}') s CPU, its helpers $(awk -v a=$((k1-k0)) -v hz=$hz 'BEGIN{printf "%.2f", a/hz}') s, PipeWire $(awk -v a=$((p1-p0)) -v hz=$hz 'BEGIN{printf "%.2f", a/hz}') s; processes started:"
$PERF script -i /tmp/ie.data -F comm 2>/dev/null | sort | uniq -c | sort -rn | head -8
