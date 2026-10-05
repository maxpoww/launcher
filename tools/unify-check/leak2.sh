# leak2.sh: which kind of recording leaves the compositor slower? The dock is stopped; the load is pointer motion.
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
PERF=/nix/var/nix/gcroots/lab/perf/bin/perf; WF=/nix/var/nix/gcroots/lab/wf-recorder/bin/wf-recorder
FF=/nix/var/nix/gcroots/lab/ffmpeg-bin/bin/ffprobe
HP=$(pgrep -x .Hyprland-wrapp | head -1)
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H"
U="runuser -u max -- env XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"
moves() { for i in $(seq 1 $1); do $E hyprctl eval "hl.dispatch(hl.dsp.cursor.move({ x = $((300+i*5%900)), y = $((300+i*3%400)) }))" >/dev/null 2>&1; done; }
measure() { $PERF stat -p $HP -e instructions -x, -o /tmp/l2.csv sleep 9999 2>/dev/null & PP=$!; sleep 0.2; moves 150; sleep 0.3; pkill -P $PP -x sleep; wait $PP 2>/dev/null
  echo "$1: $(awk -F, '/instructions/{printf "%.3f", $1/1e9}' /tmp/l2.csv) G instructions for 150 pointer moves"; }
rec() { # label, extra wf-recorder args...
  l=$1; shift
  $E $WF -y -f /tmp/leak.mkv -c libx264rgb -p crf=0 -p preset=ultrafast "$@" >/dev/null 2>&1 & sleep 1.5; moves 250; pkill -INT -x wf-recorder; sleep 2.5
  n=$($FF -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 /tmp/leak.mkv 2>/dev/null)
  measure "after $l ($n frames recorded)"
}
[ "$1" = nodock ] && { $U systemctl --user stop waverunner; sleep 2; echo "dock stopped"; }
measure "before"
rec "plain recording"
rec "plain recording again"
rec "region -g" -g "100,100 800x600"
rec "no-damage" -D
[ "$1" = nodock ] && { $U systemctl --user start waverunner; sleep 8; echo "dock started: $($U systemctl --user is-active waverunner)"; }
