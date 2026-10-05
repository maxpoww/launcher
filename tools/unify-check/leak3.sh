# leak3.sh: count the compositor's "a screen share started" announcements during recordings
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
WF=/nix/var/nix/gcroots/lab/wf-recorder/bin/wf-recorder; FF=/nix/var/nix/gcroots/lab/ffmpeg-bin/bin/ffprobe
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H"
C="$E /nix/var/nix/gcroots/lab/ctl/bin/waverunner-ctl"
moves() { for i in $(seq 1 $1); do $E hyprctl eval "hl.dispatch(hl.dsp.cursor.move({ x = $((300+i*5%900)), y = $((300+i*3%400)) }))" >/dev/null 2>&1; done; }
act() { for i in $(seq 1 $1); do $C debug-stats; sleep 0.8; $C debug-stats; sleep 0.8; done; }
listen() { perl -MIO::Socket::UNIX -e '$|=1; $s=IO::Socket::UNIX->new(Peer=>$ARGV[0]) or die; while(<$s>){print if /^screencast>>/}' /run/user/1000/hypr/$H/.socket2.sock > /tmp/sc.events & LP=$!; sleep 0.3; }
unlisten() { kill $LP 2>/dev/null; wait $LP 2>/dev/null; echo "$1: $(grep -c 'screencast>>1' /tmp/sc.events) share starts, $(grep -c 'screencast>>0' /tmp/sc.events) stops${2:+, $2}"; }
rec() { l=$1; load=$2; shift 2
  listen
  $E $WF -y -f /tmp/leak.mkv -c libx264rgb -p crf=0 -p preset=ultrafast "$@" >/dev/null 2>&1 & sleep 1.5; $load; pkill -INT -x wf-recorder; sleep 2.5
  n=$($FF -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 /tmp/leak.mkv 2>/dev/null)
  unlisten "$l" "$n frames recorded"
}
listen; act 4; unlisten "no recording, dock animating (its own samples)"
rec "plain recording, pointer moving" "moves 200"
rec "plain recording, dock animating" "act 5"
rec "region recording, pointer moving" "moves 200" -g "100,100 800x600"
