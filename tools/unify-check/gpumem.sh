export PATH=/run/current-system/sw/bin:$PATH
true; P=$(pgrep -f "bin/waverunner$"|head -1); seen=""; tot=0
for f in /proc/$P/fdinfo/*; do id=$(grep -h "drm-client-id" $f 2>/dev/null | awk '{print $2}'); [ -z "$id" ] && continue; case " $seen " in *" $id "*) continue;; esac; seen="$seen $id"; r=$(grep -h "drm-resident-system0" $f | awk '{print $2}'); [ "${r:-0}" -gt 0 ] && { echo "  gpu client $id: $((r/1024)) MB resident"; tot=$((tot+r)); }; done
echo "dock: GPU $((tot/1024)) MB, RSS $(awk '/VmRSS/{print int($2/1024)}' /proc/$P/status) MB, threads $(ls /proc/$P/task | wc -l)"
grep -E "MemAvailable|Shmem:" /proc/meminfo | tr '\n' ' '; echo
runuser -u max -- env XDG_RUNTIME_DIR=/run/user/1000 journalctl --user -u waverunner --since "-90s" -o cat | sed "s/\x1b\[[0-9;]*m//g" | grep -E "renderer:|fonts:|panic|ERROR|WARN" | cut -c30-150 | tail -8
