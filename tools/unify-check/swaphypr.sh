# usage (as root on the laptop): swaphypr.sh <store path of hyprland | old | keep>
# Runs the session's compositor from another store path until the next reboot
# (a drop-in under /run), then restarts the session (greetd's autologin again).
export PATH=/run/current-system/sw/bin:$PATH
D='/run/systemd/user/wayland-wm@hyprland\x2duwsm.desktop.service.d'
if [ "$1" != keep ]; then
  rm -rf "$D"
  if [ "$1" != old ]; then
    UW=$(readlink -f /run/current-system/sw/bin/uwsm)
    mkdir -p "$D"
    printf '[Service]\nExecStart=\nExecStart=%s aux exec -- %%I %s/bin/start-hyprland --path %s/bin/Hyprland\n' "$UW" "$1" "$1" > "$D/zz-test.conf"
  fi
fi
systemctl stop greetd
sleep 3
rm -f /run/greetd.run
systemctl start greetd
for i in $(seq 1 60); do
  sleep 2
  [ -n "$(pgrep -f 'bin/Hyprland' | head -1)" ] && [ -S /run/user/1000/waverunner.sock ] && break
done
sleep 10
# The installed compositor starts through /run/wrappers (CAP_SYS_NICE) and makes
# its main thread SCHED_RR 1; one started by path cannot. Same scheduling here.
HP=$(pgrep -x .Hyprland-wrapp | head -1)
chrt -p $HP | grep -q SCHED_RR || chrt -r -p 1 $HP
chrt -p $HP | head -1
echo "Hyprland: $(readlink /proc/$(pgrep -f 'bin/Hyprland' | head -1)/exe)"
