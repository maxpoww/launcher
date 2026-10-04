export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
G=$(ls -d /nix/store/*-grim-*/bin/grim | head -1); TAG=$1
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"
C="$E waverunner-ctl"
shot() { $E $G /tmp/soak-$TAG-$1.png; }
$E hyprctl eval 'hl.dispatch(hl.dsp.cursor.move({ x = 400, y = 500 }))' >/dev/null 2>&1
$C hide
# 1 notification
$E notify-send -a "Night test" "Soak notification" "from the unify night"; sleep 2.5; shot notif
sleep 5
# 2 clipboard
$E sh -c 'printf "hello night soak" | wl-copy >/dev/null 2>&1 </dev/null'; sleep 1.5; $C debug-clip; sleep 2.5; shot clip; $C debug-clip; sleep 1
# 3 audio stream + volume
$E wpctl set-volume @DEFAULT_AUDIO_SINK@ 30% ; sleep 1
$E sh -c 'timeout 14 pw-cat -p --raw --format s16 --rate 48000 --channels 2 /dev/zero >/dev/null 2>&1 &'
sleep 4; shot audio30
$E wpctl set-volume @DEFAULT_AUDIO_SINK@ 60%; sleep 1.5; shot audio60
sleep 10; shot audio-after
# 4 stage mode with one window
$E sh -c 'foot -D /home/max -e /run/current-system/sw/bin/sh -c "sleep 40" >/dev/null 2>&1 &'
sleep 4; $C stage-toggle; sleep 3.5; shot stage; $C stage-toggle; sleep 2.5; shot stage-off
pkill -x foot; sleep 2; shot end
runuser -u max -- env XDG_RUNTIME_DIR=/run/user/1000 journalctl --user -u waverunner --since "-80s" -o cat | sed "s/\x1b\[[0-9;]*m//g" | grep -E "panic|ERROR| WARN" | cut -c30-170 | sort | uniq -c | head -8
