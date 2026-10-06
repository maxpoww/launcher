#!/bin/bash
# options-anim.sh — how many OPTIONS frames one box animation gets, on the dev box.
# Restarts the dock through the live compositor (extra env as args, e.g. WAVERUNNER_FULL_DAMAGE=1;
# NORESTART=1 to measure the running one — it must log to /tmp/claude-1000/dock.log), opens and
# closes the gear box by script and prints the perf counters per animation. The 2026-10-06
# regression read 7–9 frames here where 60–90 is right (see docs/unify-audit-2026-10-04.md).
export XDG_RUNTIME_DIR=/run/user/1000
HC=/nix/store/2c1dwfwi8iy3s8h59q4c56i8gmqbgkcc-hyprland-0.55.4/bin/hyprctl
CTL=/home/max/launcher/target/release/waverunner-ctl
LOG=/tmp/claude-1000/dock.log
BIN=${BIN:-/home/max/launcher/waverunner-dev}
ENVS="RUST_LOG=${RUST_LOG:-info}"; for a in "$@"; do ENVS="$ENVS $a"; done
if [ -z "$NORESTART" ]; then
OLD=$(pgrep -f "release/waverunner$" | head -1); [ -n "$OLD" ] && kill $OLD; sleep 1
: > $LOG
CMD="hl.dsp.exec_cmd(\"sh -c '$ENVS exec $BIN >>$LOG 2>&1'\")"
timeout 10 $HC -i 0 dispatch "$CMD" >/dev/null 2>&1
for i in $(seq 1 30); do sleep 0.5; [ -S /run/user/1000/waverunner.sock ] && pgrep -f "release/waverunner$" >/dev/null && break; done
sleep 4
fi
HP=$(ps -C .Hyprland-wrapp -o pid= | head -1 | tr -d " "); DP=$(pgrep -f "release/waverunner$" | head -1)
cpu() { awk '{print $14+$15}' /proc/$1/stat; }
irq() { grep -E "i915|xe$|xe " /proc/interrupts | head -1 | awk '{s=0; for(i=2;i<=NF;i++) if($i ~ /^[0-9]+$/) s+=$i; print s}'; }
echo "dock pid $DP env: $ENVS"
timeout 5 $CTL debug-perf >/dev/null; h0=$(cpu $HP); d0=$(cpu $DP); i0=$(irq)
timeout 5 $CTL debug-gear open net >/dev/null; sleep 1.5; h1=$(cpu $HP); d1=$(cpu $DP); i1=$(irq); timeout 5 $CTL debug-perf >/dev/null; sleep 0.3
timeout 5 $CTL debug-gear close >/dev/null; sleep 1.5; h2=$(cpu $HP); d2=$(cpu $DP); i2=$(irq); timeout 5 $CTL debug-perf >/dev/null; sleep 0.4
echo "  open : Hyprland $((h1-h0)) dock $((d1-d0)) ticks, gpu irq $((i1-i0)) | $(grep 'perf:' $LOG | tail -n 2 | head -1 | grep -oE 'options [0-9]+ in [0-9.]+ ms|damage [^|]+' | tr '\n' ' ')"
echo "  close: Hyprland $((h2-h1)) dock $((d2-d1)) ticks, gpu irq $((i2-i1)) | $(grep 'perf:' $LOG | tail -n 1 | grep -oE 'options [0-9]+ in [0-9.]+ ms|damage [^|]+' | tr '\n' ' ')"
grep -iE "warn|error" $LOG | grep -v "present mode" | tail -n 3 | cut -c1-160
