# usage: colortest.sh <tag>  — maximized foot changes its background twice with no layout event; grim after each
export PATH=/run/current-system/sw/bin:$PATH
G=$(ls -d /nix/store/*-grim-*/bin/grim | head -1)
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=/etc/profiles/per-user/max/bin:/run/current-system/sw/bin XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H"
cat > /tmp/ct-inner.sh <<'IN'
printf '\033]11;#204060\007'; clear; sleep 7
printf '\033]11;#aa2222\007'; clear; sleep 7
printf '\033]11;#22aa44\007'; clear; sleep 60
IN
chmod 755 /tmp/ct-inner.sh
$E foot -D /home/max -e /run/current-system/sw/bin/sh /tmp/ct-inner.sh >/dev/null 2>&1 &
sleep 2; $E hyprctl eval 'hl.dispatch(hl.dsp.window.float({ action = "toggle" }))' >/dev/null; sleep 3;   $E $G -g "0,0 2049x90" /tmp/ct-$1-a.png     # initial (blue-grey)
sleep 3.2; $E $G -g "0,0 2049x90" /tmp/ct-$1-b1.png    # 1.2 s after red
sleep 2;   $E $G -g "0,0 2049x90" /tmp/ct-$1-b2.png    # 3.2 s after red
sleep 3;   $E $G -g "0,0 2049x90" /tmp/ct-$1-c1.png    # 1.2 s after green
sleep 2;   $E $G -g "0,0 2049x90" /tmp/ct-$1-c2.png    # 3.2 s after green
$E $G /tmp/ct-$1-full.png
pkill -x foot; sleep 2
$E $G -g "0,0 2049x90" /tmp/ct-$1-z.png                # back to empty desktop
ls /tmp/ct-$1-*.png | wc -l
