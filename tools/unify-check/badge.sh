# badge.sh <tag>: the stage deck's speaker badge. A terminal plays 6 s of (silent) sound, then a 1 s one, then a
# stream that is muted and unmuted; the deck is photographed through it, and every process start is timed.
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
PERF=/nix/var/nix/gcroots/lab/perf/bin/perf; CTL=/nix/var/nix/gcroots/lab/ctl/bin/waverunner-ctl; FF=/nix/var/nix/gcroots/lab/ffmpeg-bin/bin/ffmpeg
TAG=$1; G=$(ls -d /nix/store/*-grim-*/bin/grim | head -1)
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"
C="$E $CTL"
[ -f /tmp/sil6.wav ] || $FF -loglevel error -y -f lavfi -i anullsrc=r=48000:cl=stereo -t 6 /tmp/sil6.wav
[ -f /tmp/sil1.wav ] || $FF -loglevel error -y -f lavfi -i anullsrc=r=48000:cl=stereo -t 1 /tmp/sil1.wav
[ -f /tmp/sil12.wav ] || $FF -loglevel error -y -f lavfi -i anullsrc=r=48000:cl=stereo -t 12 /tmp/sil12.wav
chmod 644 /tmp/sil*.wav; rm -f /tmp/bd-$TAG-*.png /tmp/bd.go*
$E hyprctl eval "hl.dispatch(hl.dsp.cursor.move({ x = 1000, y = 500 }))" >/dev/null 2>&1; $C hide
# terminal A plays when told (a file appears); terminal B is silent
PL="mpv --ao=pulse --no-video --really-quiet --no-config"
$E sh -c "$PL /tmp/sil1.wav >/dev/null 2>&1 </dev/null"   # warm the player's files (spinning disk)
$E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c 'PL=\"$PL\"; while [ ! -e /tmp/bd.go1 ]; do sleep 0.05; done; $PL /tmp/sil6.wav; while [ ! -e /tmp/bd.go2 ]; do sleep 0.05; done; $PL /tmp/sil1.wav; while [ ! -e /tmp/bd.go3 ]; do sleep 0.05; done; $PL /tmp/sil12.wav; sleep 20' >/dev/null 2>&1 </dev/null &"; sleep 2
$E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c 'sleep 90' >/dev/null 2>&1 </dev/null &"; sleep 3
$C stage-toggle; sleep 4
$PERF record -q -a -e sched:sched_process_exec -e sched:sched_process_exit -o /tmp/bd.data sleep 9999 >/dev/null 2>&1 & PP=$!; sleep 0.5
shot() { $E $G -g "680,990 700x162" /tmp/bd-$TAG-$1.png 2>/dev/null; }
now() { date +%s.%N; }
T0=$(now); shot 00-quiet
echo "go1 $(now)"; touch /tmp/bd.go1; chmod 666 /tmp/bd.go1
for i in 01 02 03 04 05 06 07 08 09 10 11 12; do echo "s1-$i $(now)"; shot 1$i-start; done
sleep 2; shot 20-playing
sleep 1.3
for i in 01 02 03 04 05 06 07 08 09 10 11 12; do echo "e1-$i $(now)"; shot 3$i-end; done
sleep 1.5; shot 40-after
echo "go2 $(now)"; touch /tmp/bd.go2; chmod 666 /tmp/bd.go2
sleep 0.9; shot 50-chime
sleep 2.6; shot 51-chime-after
echo "go3 $(now)"; touch /tmp/bd.go3; chmod 666 /tmp/bd.go3
sleep 2.0; shot 60-long
ID=$($E pw-dump | jq -r '.[] | select(.info.props."media.class"=="Stream/Output/Audio" and .info.props."application.name"=="mpv") | .id' | head -1)
echo "stream node: $ID"
echo "mute $(now)"; $E wpctl set-mute $ID 1; sleep 1.2; shot 61-muted
echo "unmute $(now)"; $E wpctl set-mute $ID 0; sleep 1.2; shot 62-unmuted
sleep 9; shot 70-ended
pkill -P $PP -x sleep; wait $PP 2>/dev/null
$C stage-toggle; sleep 2.5; pkill -x foot; sleep 1
echo "T0 $T0"
$PERF script -i /tmp/bd.data -F comm,pid,time,event 2>/dev/null | grep -E "pw-dump|wpctl|mpv" | awk '{print $3, $1, $4}' | sed 's/sched:sched_process_//; s/://g' > /tmp/bd-$TAG.events
$PERF script -i /tmp/bd.data -F time 2>/dev/null | head -1 | awk '{print "perf-first", $1}'
cat /proc/uptime | awk '{print "uptime-now", $1}'; echo "wall-now $(now)"
