# kms.sh <tag> [scenarios]: is the REAL screen (the KMS scanout buffer, grabbed with kmsgrab) after a run of
# partial-damage frames the same as after a full redraw? No screen-capture client is involved in the first grab.
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
TAG=$1
FF=/nix/var/nix/gcroots/lab/ffmpeg-bin/bin/ffmpeg
CTL=/nix/var/nix/gcroots/lab/ctl/bin/waverunner-ctl
G=$(ls -d /nix/store/*-grim-*/bin/grim | head -1)
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"
C="$E $CTL"
ptr() { $E hyprctl eval "hl.dispatch(hl.dsp.cursor.move({ x = $1, y = $2 }))" >/dev/null 2>&1; }
park() { ptr 1000 600; }
kgrab() { $FF -y -loglevel error -f kmsgrab -device /dev/dri/card1 -i - -vf "hwdownload,format=bgr0" -frames:v 1 -update 1 /tmp/kms-$TAG-$1.png; }
read LW LH <<< "$($E hyprctl -j monitors | jq -r '.[0] | "\((.width/.scale)|floor) \((.height/.scale)|floor)"')"
CX=$((LW/2))

s_boxes()    { for i in 1 2; do $C debug-notif; sleep 0.9; ptr $((LW-100)) 120; sleep 0.5; park; sleep 1.2; $C debug-stats; sleep 0.9; ptr 120 80; sleep 0.5; park; sleep 1.2; $C debug-clip; sleep 0.9; ptr 250 120; sleep 0.5; park; sleep 1.2; done; }
s_stats()    { $C debug-clip; sleep 1.2; park; sleep 1.5; $C debug-stats; sleep 1; ptr 120 80; sleep 0.6; ptr 160 120; sleep 2; }
s_clip()     { $C debug-stats; sleep 1.2; $C debug-stats; sleep 1; $C debug-clip; sleep 1.2; ptr 250 120; sleep 0.5; ptr 300 200; sleep 2; }
s_notif()    { $C debug-notif; sleep 1.2; ptr $((LW-100)) 120; sleep 0.6; ptr $((LW-140)) 200; sleep 2; }
s_dock()     { $C show; sleep 1; for p in 1 2; do x=$((CX-160)); while [ $x -le $((CX+160)) ]; do ptr $x $((LH-40)); x=$((x+10)); done; done; ptr $((CX+40)) $((LH-40)); sleep 2; }
s_launcher() { $C show; sleep 0.8; $C expand; sleep 1.6; for q in l li lib libr libre; do $C debug-query $q; sleep 0.35; done; ptr $((CX-120)) $((LH/2)); sleep 0.4; ptr $((CX+60)) $((LH/2+40)); sleep 2; }
s_closed()   { for i in 1 2 3; do $C show; sleep 0.8; $C expand; sleep 1.6; $C collapse; sleep 0.9; $C hide; sleep 1.0; done; park; }
s_fast()     { $C fast-launch; sleep 1.3; $C fast-launch; sleep 1.0; $C fast-launch; sleep 2; }
s_panel()    { $C display show scale; sleep 2.5; ptr $((CX-100)) $((LH-200)); sleep 0.5; ptr $((CX+80)) $((LH-160)); sleep 2; }
s_stage()    { $E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c 'sleep 60' >/dev/null 2>&1 </dev/null &"; sleep 2; $E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c 'sleep 60' >/dev/null 2>&1 </dev/null &"; sleep 3; park
               $C stage-toggle; sleep 3; $C stage-pick 2; sleep 2; $C stage-toggle; sleep 3; }
leave() { case $1 in stats) $C debug-stats;; clip) $C debug-clip;; notif) $C debug-notif;; dock) $C hide;; launcher) $C collapse; sleep 0.8; $C hide;; fast) $C fast-launch;; panel) $C hide;; stage) pkill -x foot;; esac; sleep 2; park; sleep 1; }

park; $C hide; sleep 1.5
for sc in ${@:2}; do
  s_$sc
  sleep 1.5
  kgrab $sc-a                      # the real screen, as the partial frames left it
  $E $G /tmp/kms-grim.png          # a screenshot makes the compositor redraw the whole output
  sleep 0.7
  kgrab $sc-b                      # the real screen after a full redraw
  echo "$sc: grabbed"
  leave $sc
done
