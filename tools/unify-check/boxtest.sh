export PATH=/run/current-system/sw/bin:$PATH
G=$(ls -d /nix/store/*-grim-*/bin/grim | head -1); WL=/nix/var/nix/gcroots/lab/wlrctl/bin/wlrctl
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H"
C="$E /nix/var/nix/gcroots/lab/ctl/bin/waverunner-ctl"
$C show; sleep 1.5
$E hyprctl eval 'hl.dispatch(hl.dsp.cursor.move({ x = 1024, y = 1111 }))' >/dev/null; sleep 0.8
$E $WL pointer click left; sleep 3
$E $G /tmp/box-$1.png
$E hyprctl eval 'hl.dispatch(hl.dsp.cursor.move({ x = 400, y = 300 }))' >/dev/null; sleep 0.5
$E $WL pointer click left; sleep 1.5; $C hide; sleep 1
$E $G /tmp/box-$1-after.png
