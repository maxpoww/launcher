# stageshot.sh <tag>: two terminals, stage mode, pick the 2nd; screenshots of the settled deck. Run as root on the laptop.
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
CTLBIN=/nix/var/nix/gcroots/lab/ctl/bin/waverunner-ctl; TAG=$1
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"
C="$E $CTLBIN"; GRIM=$(ls -d /nix/store/*-grim-*/bin/grim | head -1)
$E hyprctl eval "hl.dispatch(hl.dsp.cursor.move({ x = 1000, y = 600 }))" >/dev/null 2>&1
$E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c 'sleep 60' >/dev/null 2>&1 </dev/null &"; sleep 2
$E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c 'sleep 60' >/dev/null 2>&1 </dev/null &"; sleep 3
$C stage-toggle; sleep 4; $E $GRIM /tmp/ss-$TAG-a.png
$C stage-pick 2; sleep 4; $E $GRIM /tmp/ss-$TAG-b.png
$C stage-pick 1; sleep 4; $E $GRIM /tmp/ss-$TAG-c.png
$C stage-toggle; sleep 2.5; pkill -x foot; sleep 1.5
