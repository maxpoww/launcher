export PATH=/run/current-system/sw/bin:$PATH
P=$(pgrep -f "bin/waverunner$"|head -1); H=$(pgrep -x .Hyprland-wrapp|head -1); N=$(pgrep -x options-notify|head -1)
echo "dock pid $P: threads $(ls /proc/$P/task | wc -l), RSS $(awk '/VmRSS/{print $2}' /proc/$P/status) kB, Pss $(awk '/^Pss:/{print $2}' /proc/$P/smaps_rollup) kB, anon $(awk '/^Pss_Anon:/{print $2}' /proc/$P/smaps_rollup) kB, file $(awk '/^Pss_File:/{print $2}' /proc/$P/smaps_rollup) kB, shmem $(awk '/^Pss_Shmem:/{print $2}' /proc/$P/smaps_rollup) kB"
snap() { for p in $P $H $N; do for t in /proc/$p/task/*; do s=$(cat $t/stat 2>/dev/null) || continue; c=$(cat $t/comm); s=${s##*) }; set -- $s; v=$(awk '/^voluntary/{print $2}' $t/status); echo "$p/$(basename $t) $c $(( ${12} + ${13} )) $v"; done; done; }
snap > /tmp/s1; sleep ${1:-60}; snap > /tmp/s2
hz=$(getconf CLK_TCK)
join <(sort /tmp/s1) <(sort /tmp/s2) | awk -v hz=$hz -v T=${1:-60} '{cpu=($6-$3)/hz; w=($7-$4); if (cpu>0 || w>0) printf "%-22s %-18s cpu %.2fs (%.2f%%)  wakeups %.1f/s\n", $1, $2, cpu, 100*cpu/T, w/T}' | sort -k7 -t' ' | sort -t'/' -k1,1 -s
echo "totals:"; join <(sort /tmp/s1) <(sort /tmp/s2) | awk -v hz=$hz -v T=${1:-60} '{split($1,a,"/"); cpu[a[1]]+=($6-$3)/hz; w[a[1]]+=($7-$4)} END{for (p in cpu) printf "  pid %s: cpu %.2f%%  wakeups %.1f/s\n", p, 100*cpu[p]/T, w[p]/T}'
