# whodraws.sh <state> [seconds]: in a resting state, where on the screen is the damage that wakes the dock's capture?
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
CTLBIN=/nix/var/nix/gcroots/lab/ctl/bin/waverunner-ctl; s=$1; N=${2:-15}
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"
C="$E $CTLBIN"; ev() { $E hyprctl eval "$1" >/dev/null 2>&1; }
foot_() { $E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c '$1' >/dev/null 2>&1 </dev/null &"; }
in_overview() { foot_ "sleep $((N+25))"; sleep 2; foot_ "sleep $((N+25))"; sleep 3; ev 'hl.plugin.waveview.toggle()'; }; out_overview() { ev 'hl.plugin.waveview.toggle()'; sleep 1.5; pkill -x foot; }
in_spread() { foot_ "sleep $((N+25))"; sleep 2; foot_ "sleep $((N+25))"; sleep 3; ev 'hl.plugin.waveview.spread()'; }; out_spread() { ev 'hl.plugin.waveview.close()'; sleep 1.5; pkill -x foot; }
in_music() { foot_ "mpv --ao=pulse --no-video --really-quiet --no-config --loop=inf /tmp/sil12.wav"; }; out_music() { pkill -x mpv; sleep 0.5; pkill -x foot; }
in_twofoot() { foot_ "sleep $((N+25))"; sleep 2; foot_ "sleep $((N+25))"; }; out_twofoot() { pkill -x foot; }
in_onefoot() { foot_ "sleep $((N+25))"; }; out_onefoot() { pkill -x foot; }
in_rest() { :; }; out_rest() { :; }
in_mpvbare() { $E sh -c "mpv --ao=pulse --no-video --really-quiet --no-config --loop=inf /tmp/sil12.wav >/dev/null 2>&1 </dev/null &"; }; out_mpvbare() { pkill -x mpv; }
in_pwplay() { foot_ "pw-play /tmp/sil12.wav; pw-play /tmp/sil12.wav; pw-play /tmp/sil12.wav"; }; out_pwplay() { pkill -x pw-play; pkill -x foot; }
in_mpvnompris() { foot_ "mpv --ao=pulse --no-video --really-quiet --no-config --load-scripts=no --script-opts=mpris-off=1 --loop=inf /tmp/sil12.wav"; }; out_mpvnompris() { pkill -x mpv; sleep 0.5; pkill -x foot; }
ev 'hl.dispatch(hl.dsp.cursor.move({ x = 1000, y = 500 }))'; $C hide; sleep 1
in_$s; sleep 5; $C debug-perf; sleep 0.3; T0=$(date "+%F %T"); sleep $N
$C debug-perf; sleep 0.3
echo "== $s: damage that woke a capture, over $N s (count  WxH at X,Y)"
runuser -u max -- env XDG_RUNTIME_DIR=/run/user/1000 journalctl --user -u waverunner --since "$T0" -o cat | sed "s/\x1b\[[0-9;]*m//g" | grep "perf: " | tail -1 | sed 's/.*perf: //' | cut -c1-300
runuser -u max -- env XDG_RUNTIME_DIR=/run/user/1000 journalctl --user -u waverunner --since "$T0" -o cat | sed "s/\x1b\[[0-9;]*m//g" | grep "capture woken" | sed 's/.*damage //' | sort | uniq -c | sort -rn | head -8
$E hyprctl -j clients | jq -r '.[] | "   window \(.class) at \(.at[0]),\(.at[1]) size \(.size[0])x\(.size[1]) ws \(.workspace.id)"' | head -4
out_$s; sleep 2
