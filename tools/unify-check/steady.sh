# steady.sh [seconds] [states...]: what each RESTING state of the shell costs — a state is entered, left alone, measured.
# Columns: dock CPU (its threads), helpers (child processes it started), Hyprland CPU, GPU busy, frames the dock drew, processes started.
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
PERF=/nix/var/nix/gcroots/lab/perf/bin/perf; CTLBIN=/nix/var/nix/gcroots/lab/ctl/bin/waverunner-ctl
N=${1:-20}; shift; ST="${@:-rest dock launcher notif stats clip panel fast stage overview spread music locked}"
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"
C="$E $CTLBIN"; ev() { $E hyprctl eval "$1" >/dev/null 2>&1; }
ptr() { ev "hl.dispatch(hl.dsp.cursor.move({ x = $1, y = $2 }))"; }
foot_() { $E sh -c "foot -D /home/max -e /run/current-system/sw/bin/sh -c '$1' >/dev/null 2>&1 </dev/null &"; }
DP=$(pgrep -f "bin/waverunner$" | head -1); HP=$(pgrep -x .Hyprland-wrapp | head -1); hz=$(getconf CLK_TCK)
cpu() { awk '{print $14+$15}' /proc/$1/stat; }; kids() { awk '{print $16+$17}' /proc/$1/stat; }
read LW LH <<< "$($E hyprctl -j monitors | jq -r '.[0] | "\((.width/.scale)|floor) \((.height/.scale)|floor)"')"
in_rest() { :; };                  out_rest() { :; }
in_dockhover() { $C show; };       out_dockhover() { ptr 1000 500; sleep 0.5; $C hide; };       at_dockhover() { ptr $((LW/2-60)) $((LH-40)); }
in_barhover() { :; };              out_barhover() { ptr 1000 500; };                           at_barhover() { ptr $((LW/2)) 12; }
in_launchhover() { $C show; sleep 0.8; $C expand; }; out_launchhover() { ptr 1000 500; $C collapse; sleep 0.8; $C hide; }; at_launchhover() { ptr $((LW/2-100)) $((LH/2)); }
in_gearhover() { :; };             out_gearhover() { ptr 1000 500; };                          at_gearhover() { ptr 40 12; }
in_dock() { $C show; };            out_dock() { $C hide; }
in_launcher() { $C show; sleep 0.8; $C expand; }; out_launcher() { $C collapse; sleep 0.8; $C hide; }
in_notif() { $C debug-notif; };    out_notif() { $C debug-notif; }
in_stats() { $C debug-stats; };    out_stats() { $C debug-stats; }
in_clip() { $C debug-clip; };      out_clip() { $C debug-clip; }
in_panel() { $C display show scale; }; out_panel() { $C hide; }
in_fast() { $C fast-launch; };     out_fast() { $C fast-launch; }
in_stage() { foot_ "sleep $((N+25))"; sleep 2; foot_ "sleep $((N+25))"; sleep 3; $C stage-toggle; }; out_stage() { $C stage-toggle; sleep 2.5; pkill -x foot; }
in_overview() { foot_ "sleep $((N+25))"; sleep 2; foot_ "sleep $((N+25))"; sleep 3; ev 'hl.plugin.waveview.toggle()'; }; out_overview() { ev 'hl.plugin.waveview.toggle()'; sleep 1.5; pkill -x foot; }
in_spread() { foot_ "sleep $((N+25))"; sleep 2; foot_ "sleep $((N+25))"; sleep 3; ev 'hl.plugin.waveview.spread()'; }; out_spread() { ev 'hl.plugin.waveview.close()'; sleep 1.5; pkill -x foot; }
in_music() { [ -f /tmp/sil12.wav ] || /nix/var/nix/gcroots/lab/ffmpeg-bin/bin/ffmpeg -loglevel error -y -f lavfi -i anullsrc=r=48000:cl=stereo -t 12 /tmp/sil12.wav; chmod 644 /tmp/sil12.wav
  foot_ "mpv --ao=pulse --no-video --really-quiet --no-config --loop=inf /tmp/sil12.wav"; }; out_music() { pkill -x mpv; sleep 0.5; pkill -x foot; }
in_locked() { ev 'hl.dispatch(hl.dsp.exec_cmd("hyprlock"))'; }; out_locked() { pkill -USR1 -x hyprlock; sleep 2; }
ptr 1000 500; $C hide; sleep 2
printf "%-9s %8s %8s %8s %8s %9s %7s  %s\n" state dock_cpu helpers hypr_cpu gpu_busy "% 1 core" started frames
for s in $ST; do
  in_$s; sleep 5; if declare -F at_$s >/dev/null; then at_$s; sleep 4; else ptr 1000 500; fi
  $C debug-perf; sleep 0.3
  T0=$(date "+%F %T"); d0=$(cpu $DP); k0=$(kids $DP); h0=$(cpu $HP)
  $PERF stat -a -e i915/rcs0-busy/ -e sched:sched_process_exec -x, -o /tmp/st.$s sleep $N 2>/dev/null
  d1=$(cpu $DP); k1=$(kids $DP); h1=$(cpu $HP)
  $C debug-perf; sleep 0.4
  line=$(runuser -u max -- env XDG_RUNTIME_DIR=/run/user/1000 journalctl --user -u waverunner --since "$T0" -o cat | sed "s/\x1b\[[0-9;]*m//g" | grep "perf: " | tail -1 | sed 's/.*perf: //')
  fr=$(echo "$line" | sed -E 's/.*(dock [0-9]+) in [^|]*\| (options [0-9]+) in [^|]*\| (deck [0-9]+) in.*/\1, \2, \3/')
  cap=$(echo "$line" | grep -o "captures [0-9]*")
  busy=$(grep -m1 rcs0-busy /tmp/st.$s | cut -d, -f1); ex=$(grep -m1 sched_process_exec /tmp/st.$s | cut -d, -f1)
  awk -v s=$s -v n=$N -v hz=$hz -v d=$((d1-d0)) -v k=$((k1-k0)) -v h=$((h1-h0)) -v busy="$busy" -v ex="$ex" -v fr="$fr; $cap" 'BEGIN{printf "%-9s %7.2fs %7.2fs %7.2fs %7.1f%% %8.1f%% %7d  %s\n", s, d/hz, k/hz, h/hz, 100*busy/1e9/n, 100*(d+k+h)/hz/n, ex-1, fr}'
  [ -n "$FULL" ] && echo "    $line" | cut -c1-330
  [ -n "$FULL" ] && { $PERF record -q -a -e sched:sched_process_exec -o /tmp/st.data sleep 6 >/dev/null 2>&1; echo "    started in 6 s: $($PERF script -i /tmp/st.data -F comm 2>/dev/null | sort | uniq -c | sort -rn | head -6 | awk '{printf "%s×%s ", $1, $2}')"; }
  out_$s; sleep 3
done
[ "$(pgrep -f "bin/waverunner$" | head -1)" = "$DP" ] || echo "DOCK RESTARTED during the run"
