# deep.sh <tag>: the second-pass scenarios. Run as root on the laptop.
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
TAG=$1; G=$(ls -d /nix/store/*-grim-*/bin/grim | head -1)
env_() { W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
  E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"; C="$E waverunner-ctl"; }
env_
ev() { $E hyprctl eval "$1" >/dev/null 2>&1; }
shot() { $E $G /tmp/deep-$TAG-$1.png 2>/dev/null; }
dockpid() { pgrep -f "bin/waverunner$" | head -1; }
P0=$(dockpid); STEP_T=$(date "+%F %T")
caps() { # captures over $1 seconds (brain thread sees every screencast event)
  local P=$(dockpid) T; T=$(for t in /proc/$P/task/*; do [ "$(cat $t/comm 2>/dev/null)" = options-brain ] && basename $t; done)
  timeout $1 strace -p $T -o /tmp/deep.strace -s 40 -e trace=recvfrom 2>/dev/null; grep -c 'screencast>>1' /tmp/deep.strace; }
check() { # check <name> [expect-restart]
  local p=$(dockpid) errs st="ok"
  errs=$(runuser -u max -- env XDG_RUNTIME_DIR=/run/user/1000 journalctl --user -u waverunner --since "$STEP_T" -o cat | sed "s/\x1b\[[0-9;]*m//g" | grep -E "panicked|ERROR| WARN" | grep -v "no adapter via gpu" | cut -c30-140 | sort | uniq -c | head -4)
  [ -z "$p" ] && st="DOCK DEAD"
  [ -n "$p" ] && [ "$p" != "$P0" ] && [ -z "$2" ] && st="DOCK RESTARTED ($P0 -> $p)"
  [ -n "$errs" ] && st="$st; log: $errs"
  echo "[$1] $st"; P0=$(dockpid); STEP_T=$(date "+%F %T"); }
mon() { $E hyprctl -j monitors | jq -r '.[0] | "\(.width)x\(.height) scale \(.scale)"'; }
foot_() { $E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c 'sleep $1' >/dev/null 2>&1 </dev/null &"; }
ev 'hl.dispatch(hl.dsp.cursor.move({ x = 400, y = 400 }))'; $C hide
echo "start: dock pid $P0, $(mon), golem $(nixos-version --configuration-revision | cut -c1-8)"

# 1 suspend / resume
rtcwake -m mem -s 25 >/dev/null 2>&1; sleep 10; env_; shot 01-resume; $C show; sleep 2; shot 01-resume-dock; $C hide
echo "   captures in 20 s after resume: $(caps 20)"; check "1 suspend/resume"

# 2 scale change (dissolve uses its own screen capture) and back
S0=$(jq -r '.displays | to_entries[0].value.scale' /home/max/.config/golem/settings.json 2>/dev/null); [ -z "$S0" -o "$S0" = null ] && S0=$($E hyprctl -j monitors | jq -r '.[0].scale')
$C display scale 1; sleep 4; echo "   scale -> $(mon)"; shot 02-scale1; $C display scale $S0; sleep 4; echo "   back  -> $(mon)"; shot 02-scale-back; check "2 scale change and back"

# 3 fullscreen: captures must pause, then resume
foot_ 30; sleep 3; ev 'hl.dispatch(hl.dsp.window.fullscreen({ action = "toggle" }))'; sleep 3; shot 03-fullscreen
echo "   captures in 12 s under fullscreen: $(caps 12)"; ev 'hl.dispatch(hl.dsp.window.fullscreen({ action = "toggle" }))'; sleep 3; shot 03-fullscreen-off; pkill -x foot; sleep 2; check "3 fullscreen"

# 4 overview and spread (the plugin) with two windows
foot_ 40; sleep 2; foot_ 40; sleep 3
ev 'hl.plugin.waveview.toggle()'; sleep 2.5; shot 04-overview; ev 'hl.plugin.waveview.toggle()'; sleep 2
ev 'hl.plugin.waveview.spread()'; sleep 2.5; shot 04-spread; ev 'hl.plugin.waveview.close()'; sleep 2; shot 04-closed; check "4 overview + spread"

# 5 stage with two windows, pick the other
$C stage-toggle; sleep 3.5; shot 05-stage; $C stage-pick 2; sleep 3; shot 05-stage-pick; $C stage-toggle; sleep 3; check "5 stage + deck"

# 6 minimize to dock and restore
A=$($E hyprctl -j activewindow | jq -r .address); ev "hl.plugin.waveview.minimize(\"$A\")"; sleep 3; $C show; sleep 2; shot 06-minimized
ev "hl.plugin.waveview.restore_min(\"$A\")"; sleep 3; shot 06-restored; $C hide; check "6 minimize / restore"

# 7 rapid workspace switching (capture aborts, zone re-evaluations)
for i in 2 3 1 2 4 1 3 2 1 2 3 1; do ev "hl.dispatch(hl.dsp.focus({ workspace = $i }))"; sleep 0.25; done; ev 'hl.dispatch(hl.dsp.focus({ workspace = 1 }))'; sleep 3; shot 07-ws; pkill -x foot; sleep 2; check "7 rapid workspace switches"

# 8 lock and unlock (hyprlock as the compositor's child)
ev 'hl.dispatch(hl.dsp.exec_cmd("hyprlock"))'; sleep 6; LK=$(pgrep -x hyprlock | wc -l); pkill -USR1 -x hyprlock; sleep 4; shot 08-unlocked; echo "   hyprlock ran: $LK, still locked: $(pgrep -x hyprlock | wc -l)"; check "8 lock / unlock"

# 9 notification burst (icon array rebuilt on every arrival)
for a in firefox org.gnome.Nautilus vlc audacity libreoffice-writer Golem; do $E notify-send -a "$a" -i "$a" "Burst $a" "deep check"; sleep 0.3; done; sleep 2; $C debug-notif; sleep 3; shot 09-notifs; $C debug-notif; sleep 1; check "9 notification burst"

# 10 clipboard images (thumbnails share that array)
for n in 01-resume 02-scale1; do $E sh -c "wl-copy -t image/png < /tmp/deep-$TAG-$n.png >/dev/null 2>&1 </dev/null"; sleep 1.5; done; $C debug-clip; sleep 3; shot 10-clip; $C debug-clip; sleep 3; shot 10-clip-b; pkill -x wl-copy; check "10 clipboard images"

# 11 screen off / on
ev 'hl.dispatch(hl.dsp.dpms({ action = "off" }))'; sleep 6; ev 'hl.dispatch(hl.dsp.dpms({ action = "on" }))'; sleep 4; shot 11-dpms; check "11 screen off/on"

# 12 the dock killed (-9): must come back by itself and draw
kill -9 $(dockpid); sleep 14; $C show; sleep 2; shot 12-after-kill; $C hide; check "12 dock killed" restart

# 13 the compositor crashes once: desktop restarts, dock with it
kill -SEGV $(pgrep -x .Hyprland-wrapp | head -1); sleep 25; env_; $C show; sleep 2; shot 13-after-crash; $C hide; echo "   Hyprland up: $(pgrep -x .Hyprland-wrapp | wc -l)"; check "13 compositor crash" restart

# 14 launcher + panel + fast launch once more after all that
$C show; sleep 1; $C expand; sleep 3.5; shot 14-open; $C collapse; $C hide; sleep 1; $C fast-launch; sleep 2; shot 14-fast; $C fast-launch; check "14 launcher after everything"
echo "end: $(mon); captures in 30 s at rest: $(caps 30)"
P=$(dockpid); tot=0; pur=0; seen=""; for f in /proc/$P/fdinfo/*; do id=$(grep -h "drm-client-id" $f 2>/dev/null | awk '{print $2}'); [ -z "$id" ] && continue; case " $seen " in *" $id "*) continue;; esac; seen="$seen $id"; tot=$((tot+$(grep -h "drm-resident-system0" $f | awk '{print $2+0}'))); pur=$((pur+$(grep -h "drm-purgeable-system0" $f | awk '{print $2+0}'))); done
echo "dock: RSS $(awk '/VmRSS/{print int($2/1024)}' /proc/$P/status) MB, GPU in use $(((tot-pur)/1024)) MB (+$((pur/1024)) MB reclaimable cache)"
