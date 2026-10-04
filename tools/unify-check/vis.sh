# vis.sh <tag>: screenshot a set of shell states
export PATH=/run/current-system/sw/bin:$PATH
G=$(ls -d /nix/store/*-grim-*/bin/grim | head -1)
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H"
C="$E /etc/profiles/per-user/max/bin/waverunner-ctl"
shot() { sleep ${2:-2.5}; $E $G /tmp/vis-$TAG-$1.png; }
TAG=$1
# park the pointer in a fixed neutral spot so hover never differs
$E hyprctl eval 'hl.dispatch(hl.dsp.cursor.move({ x = 400, y = 300 }))' >/dev/null 2>&1
$C hide; shot rest 2
$C show; shot dock
$C expand; shot open 3.5
$C collapse; $C hide; sleep 1
$C debug-stats; shot stats; $C debug-stats; sleep 1
$C debug-clip; shot clip; $C debug-clip; sleep 1
$C debug-notif; shot notif; $C debug-notif; sleep 1
$C display show scale; shot panel 3.5; $C hide; sleep 1
$C fast-launch; shot fast; $C fast-launch; sleep 1
$C hide
ls /tmp/vis-$TAG-*.png | wc -l
