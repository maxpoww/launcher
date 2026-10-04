export PATH=/run/current-system/sw/bin:$PATH
U="runuser -u max -- env XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"
for i in 1 2; do
  $U systemctl --user stop waverunner; sync; echo 3 > /proc/sys/vm/drop_caches; sleep 1
  since=$(date "+%F %T"); sleep 1; t0=$(cut -d' ' -f1 /proc/uptime); $U systemctl --user start waverunner
  until $U journalctl --user -u waverunner --since "$since" -o cat | grep -q "app index ready"; do sleep 0.5; done; sleep 1
  $U journalctl --user -u waverunner --since "$since" -o short-monotonic | sed "s/\x1b\[[0-9;]*m//g" | awk -F"[][]" -v t0=$t0 '/daemon up|adapter via|sharing the GPU|app index ready|fonts:/ { n=$0; sub(/.*INFO /,"",n); printf "  +%.1fs %s\n", $2-t0, substr(n,1,58) }'
  echo
done
