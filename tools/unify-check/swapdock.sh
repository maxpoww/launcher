# usage: swapdock.sh <store path of waverunner-daemon | old>
export PATH=/run/current-system/sw/bin:$PATH
U='runuser -u max -- env XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus'
D=/run/user/1000/systemd/user/waverunner.service.d
rm -f $D/zz-night-test.conf
$U systemctl --user daemon-reload
if [ "$1" != old ]; then
  V=$($U systemctl --user cat waverunner | grep '^ExecStart=/' | awk '{print $1}' | cut -d= -f2)
  mkdir -p $D; printf '[Service]\nExecStart=\nExecStart=%s %s/bin/waverunner\n' "$V" "$1" > $D/zz-night-test.conf
  chown -R max: /run/user/1000/systemd; $U systemctl --user daemon-reload
fi
$U systemctl --user restart waverunner
sleep ${2:-35}
$U systemctl --user cat waverunner | grep ^ExecStart | tail -1 | cut -c1-150
