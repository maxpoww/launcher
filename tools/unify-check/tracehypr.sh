# tracehypr.sh on|off : restart the session with Hyprland trace logging (a /run drop-in, gone at reboot)
export PATH=/run/current-system/sw/bin:$PATH
D='/run/systemd/user/wayland-wm@hyprland\x2duwsm.desktop.service.d'
rm -f "$D/zz-trace.conf"; rmdir "$D" 2>/dev/null
if [ "$1" = on ]; then mkdir -p "$D"; printf '[Service]\nEnvironment=HYPRLAND_TRACE=1\n' > "$D/zz-trace.conf"; fi
systemctl stop greetd; sleep 3; rm -f /run/greetd.run; systemctl start greetd
for i in $(seq 1 60); do sleep 2; [ -n "$(pgrep -x .Hyprland-wrapp | head -1)" ] && [ -S /run/user/1000/waverunner.sock ] && break; done
sleep 10
HP=$(pgrep -x .Hyprland-wrapp | head -1); echo "Hyprland $HP: $(tr '\0' '\n' < /proc/$HP/environ | grep -c HYPRLAND_TRACE) trace env; $(chrt -p $HP | head -1 | sed 's/.*: //')"
