#!/usr/bin/env bash
# THE DOCK BUSY, against a budget: the boxes, the gear, the clipboard, a Files window and the
# quick controls opened and closed N times, then the loop's own meter is read. Exit 1 when a
# turn of the loop took long enough to be felt more than the budget allows, or the dock died.
#
#   tools/perf-guard/churn-check.sh [rounds=6] [max stalls over 30 ms=rounds] [worst ms=100]
#
# Use a rig (or a machine nobody is using): it opens things on the screen. WAVERUNNER_LOG and
# CTL say where the log and waverunner-ctl are. Dev box, 3200x2000, 2026-10-30: 4 stalls in 6
# rounds, worst 39.6 ms. A slow machine gets a looser worst through the third argument.
ROUNDS=${1:-6}; MAX_STALLS=${2:-$ROUNDS}; MAX_WORST=${3:-100}
LOG=${WAVERUNNER_LOG:-${XDG_STATE_HOME:-$HOME/.local/state}/waverunner/daemon.log}
CTL=${CTL:-waverunner-ctl}
"$CTL" debug-perf >/dev/null 2>&1; sleep 1
for _ in $(seq "$ROUNDS"); do
  "$CTL" expand >/dev/null 2>&1; sleep .5; "$CTL" debug-query fire >/dev/null 2>&1; sleep .4
  "$CTL" debug-query "" >/dev/null 2>&1; "$CTL" hide >/dev/null 2>&1; sleep .4
  "$CTL" debug-gear "open cpu" >/dev/null 2>&1; sleep .5; "$CTL" debug-gear close >/dev/null 2>&1; sleep .3
  "$CTL" debug-clip >/dev/null 2>&1; sleep .5; "$CTL" debug-clip >/dev/null 2>&1; sleep .3
  "$CTL" files /tmp >/dev/null 2>&1; sleep .6; "$CTL" files close >/dev/null 2>&1; sleep .3
  "$CTL" fast-controls >/dev/null 2>&1; sleep .5; "$CTL" fast-controls >/dev/null 2>&1; sleep .3
done
sleep 2; "$CTL" debug-perf >/dev/null 2>&1 || { echo "OVER BUDGET: the dock does not answer"; exit 1; }
sleep 1
line=$(grep 'perf: loop' "$LOG" | tail -1)
turns=$(sed -E 's/.*perf: loop ([0-9]+) turns.*/\1/' <<<"$line")
stalls=$(sed -E 's/.* ([0-9]+) stalls >30 ms.*/\1/' <<<"$line")
worst=$(sed -E 's/.*worst ([0-9.]+)\).*/\1/' <<<"$line")
echo "$ROUNDS rounds: $turns turns of the loop, $stalls over 30 ms, worst $worst ms"
if [ "$stalls" -gt "$MAX_STALLS" ] || [ "${worst%.*}" -ge "$MAX_WORST" ]; then
  echo "OVER BUDGET (at most $MAX_STALLS stalls, worst under $MAX_WORST ms)"; exit 1
fi
echo "within budget"
