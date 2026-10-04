export PATH=/run/current-system/sw/bin:$PATH
P=$(pgrep -f "bin/waverunner$"|head -1); T=$(for t in /proc/$P/task/*; do [ "$(cat $t/comm)" = options-brain ] && basename $t; done)
timeout ${1:-30} strace -tt -p $T -o /tmp/brain.strace -s 60 -yy 2>/dev/null
echo "captures in ${1:-30}s at rest: $(grep -c 'screencast>>1' /tmp/brain.strace)  at: $(grep 'screencast>>1' /tmp/brain.strace | awk '{print $1}' | cut -c4-10 | tr '\n' ' ')"
