# audiowatch.sh: the brain's audio sensor after outside changes — volume, default sink, a client that changes nothing,
# the watcher killed. Prints every probe (pw-dump / wpctl started by the dock) against the moment of each action.
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
PERF=/nix/var/nix/gcroots/lab/perf/bin/perf
U="runuser -u max -- env XDG_RUNTIME_DIR=/run/user/1000 HOME=/home/max PATH=$PATH DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"
DP=$(pgrep -f "bin/waverunner$" | head -1)
V0=$($U wpctl get-volume @DEFAULT_AUDIO_SINK@ | awk '{print $2}'); SINK=$($U wpctl inspect @DEFAULT_AUDIO_SINK@ | head -1 | grep -o "id [0-9]*" | awk '{print $2}')
echo "volume before: $V0, default sink id: $SINK"; sleep 2
$PERF record -q -a -e sched:sched_process_exec -e sched:sched_process_fork -o /tmp/aw.data sleep 9999 >/dev/null 2>&1 & PP=$!; sleep 1.5
mark() { echo "MARK $1 $(awk '{print $1}' /proc/uptime) $(date +%s.%N)"; }
mark start
sleep 3; mark volume-up;   $U wpctl set-volume @DEFAULT_AUDIO_SINK@ 0.33
sleep 3; mark volume-back; $U wpctl set-volume @DEFAULT_AUDIO_SINK@ $V0
sleep 3; mark set-default; $U wpctl set-default $SINK
sleep 3; mark foreign-dump; $U pw-dump > /dev/null
sleep 3; mark mute-sink;   $U wpctl set-mute @DEFAULT_AUDIO_SINK@ 1
sleep 2; mark unmute-sink; $U wpctl set-mute @DEFAULT_AUDIO_SINK@ 0
sleep 3; mark kill-pw-mon; MON=$(pgrep -P $DP -x pw-mon | head -1); kill $MON
sleep 9; mark end
pkill -P $PP -x sleep; wait $PP 2>/dev/null
echo "volume after: $($U wpctl get-volume @DEFAULT_AUDIO_SINK@)"
# probes = pw-dump / wpctl whose parent is the dock's brain thread
$PERF script -i /tmp/aw.data -F comm,pid,tid,time,event,trace 2>/dev/null > /tmp/aw.txt
grep "sched_process_fork" /tmp/aw.txt | grep "comm=options-brain" | sed -E 's/.* ([0-9]+\.[0-9]+): .*child_pid=([0-9]+)/\1 \2/' > /tmp/aw.kids
awk 'NR==FNR{kid[$2]=1; next} /sched_process_exec/ { for(i=1;i<=NF;i++) if ($i ~ /^pid=/) { split($i,a,"="); p=a[2] } t=""; for(i=1;i<=NF;i++) if ($i ~ /^[0-9]+\.[0-9]+:$/) t=$i; n=$0; sub(/.*filename=/,"",n); sub(/ .*/,"",n); sub(/.*\//,"",n); if (kid[p]) print t, "probe", n; else if (n ~ /wpctl|pw-dump/) print t, "outside", n }' /tmp/aw.kids /tmp/aw.txt | sed 's/://' > /tmp/aw.ev
head -1 /tmp/aw.txt | awk '{for(i=1;i<=NF;i++) if ($i ~ /^[0-9]+\.[0-9]+:$/) print "PERF0", $i}' | sed 's/://'
cat /tmp/aw.ev
