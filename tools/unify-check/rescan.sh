export PATH=/run/current-system/sw/bin:$PATH
G=$(ls -d /nix/store/*-grim-*/bin/grim | head -1)
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H"
C="$E /etc/profiles/per-user/max/bin/waverunner-ctl"
$E hyprctl eval 'hl.dispatch(hl.dsp.cursor.move({ x = 400, y = 300 }))' >/dev/null 2>&1
$C expand; sleep 3.5; $E $G /tmp/rs-a.png; $C collapse; $C hide; sleep 1
$E sh -c 'mkdir -p ~/.local/share/applications; printf "[Desktop Entry]\nType=Application\nName=Zz Night Test\nExec=true\nIcon=utilities-terminal\n" > ~/.local/share/applications/zz-night-test.desktop'
sleep 6
$E rm -f /home/max/.local/share/applications/zz-night-test.desktop
sleep 6
$C show; sleep 1.5; $C expand; sleep 3.5; $E $G /tmp/rs-b.png; $C collapse; $C hide
runuser -u max -- env XDG_RUNTIME_DIR=/run/user/1000 journalctl --user -u waverunner --since "-40s" -o cat | sed "s/\x1b\[[0-9;]*m//g" | grep -E "app index ready|panic" | cut -c30-100
P=$(pgrep -f "bin/waverunner$"|head -1); grep -E "^Rss:" /proc/$P/smaps_rollup
