export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
G=$(ls -d /nix/store/*-grim-*/bin/grim | head -1); TAG=$1
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"
shot() { $E $G -g "0,0 2049x60" /tmp/au-$TAG-$1.png; }
$E wpctl set-volume @DEFAULT_AUDIO_SINK@ 30%; sleep 2; shot idle
$E sh -c 'timeout 16 pw-cat -p --format s16 --rate 48000 --channels 2 /dev/urandom >/dev/null 2>&1 < /dev/null &' 
sleep 1.2; shot play1     # 1.2 s after the stream starts
sleep 2;   shot play3
$E wpctl set-volume @DEFAULT_AUDIO_SINK@ 55%; sleep 0.8; shot vol55-08
sleep 1.5; shot vol55-23
$E wpctl set-volume @DEFAULT_AUDIO_SINK@ 0.05; sleep 0.8; shot vol05-08
sleep 12; shot after      # stream ended ~
$E wpctl set-volume @DEFAULT_AUDIO_SINK@ 30%
