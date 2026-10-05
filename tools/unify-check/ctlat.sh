# ctlat.sh <tag>: how soon the bar follows a tiled window's colour change (no layout event)
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
G=$(ls -d /nix/store/*-grim-*/bin/grim | head -1); W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H"
$E hyprctl eval 'hl.dispatch(hl.dsp.cursor.move({ x = 1024, y = 700 }))' >/dev/null
T1=$(( $(date +%s) + 9 )); T2=$((T1 + 6))
cat > /tmp/ct-inner.sh <<IN
printf '\033]11;#204060\007'; clear
while [ \$(date +%s) -lt $T1 ]; do sleep 0.02; done; printf '\033]11;#aa2222\007'; clear
while [ \$(date +%s) -lt $T2 ]; do sleep 0.02; done; printf '\033]11;#22aa44\007'; clear
sleep 8
IN
chmod 755 /tmp/ct-inner.sh
$E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh /tmp/ct-inner.sh >/dev/null 2>&1 </dev/null &"
sleep 2; $E hyprctl eval 'hl.dispatch(hl.dsp.window.float({ action = "toggle" }))' >/dev/null
px() { $E $G -g "300,12 1x1" -t ppm - 2>/dev/null | tail -c 3 | od -An -tu1 | tr -s ' ' ','; }
for T in $T1 $T2; do
  while [ $(date +%s) -lt $T ]; do sleep 0.01; done
  for d in 0.10 0.15 0.25 0.4 0.6; do sleep $d; echo "$1 change@$T +$(awk -v n=$(date +%s.%N) -v t=$T 'BEGIN{printf "%.2f", n-t}')s bar=$(px)"; done
done
pkill -x foot; sleep 1
