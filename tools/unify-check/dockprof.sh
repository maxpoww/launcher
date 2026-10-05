# dockprof.sh <workload fn>: sample the dock's main thread during a workload
export PATH=/run/current-system/sw/bin:/etc/profiles/per-user/max/bin:$PATH
PERF=/nix/var/nix/gcroots/lab/perf/bin/perf; CTL=/nix/var/nix/gcroots/lab/ctl/bin/waverunner-ctl
W=$(ls /run/user/1000 | grep -m1 "^wayland-[0-9]$"); H=$(ls -t /run/user/1000/hypr | head -1)
E="runuser -u max -- env HOME=/home/max PATH=$PATH XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=$W HYPRLAND_INSTANCE_SIGNATURE=$H"
C="$E $CTL"; DP=$(pgrep -f "bin/waverunner$" | head -1)
ptr() { $E hyprctl eval "hl.dispatch(hl.dsp.cursor.move({ x = $1, y = $2 }))" >/dev/null 2>&1; }
read LW LH <<< "$($E hyprctl -j monitors | jq -r '.[0] | "\((.width/.scale)|floor) \((.height/.scale)|floor)"')"; CX=$((LW/2))
wl_panel() { for i in 1 2 3; do $C display show scale; sleep 2.2; $C hide; sleep 1.3; done; }
wl_launcher() { for i in 1 2 3 4; do $C show; sleep 0.8; $C expand; sleep 1.6; $C collapse; sleep 0.9; $C hide; sleep 1.0; done; }
wl_hover() { $C show; sleep 1; for s in 1 2 3; do x=$((CX-160)); while [ $x -le $((CX+160)) ]; do ptr $x $((LH-40)); x=$((x+8)); done; done; ptr 1000 600; sleep 0.5; $C hide; sleep 1; }
wl_boxes() { for i in 1 2 3; do $C debug-notif; sleep 0.9; ptr $((LW-100)) 120; sleep 0.5; ptr 1000 600; sleep 1.3; $C debug-stats; sleep 0.9; ptr 120 80; sleep 0.5; ptr 1000 600; sleep 1.3; $C debug-clip; sleep 0.9; ptr 250 120; sleep 0.5; ptr 1000 600; sleep 1.3; done; }
ptr 1000 600; $C hide; sleep 1
$PERF record -q -g -t $DP -o /tmp/dp.data sleep 9999 >/dev/null 2>&1 & PP=$!; sleep 0.3
wl_$1
pkill -P $PP -x sleep; wait $PP 2>/dev/null
echo "== $1: where the dock's main thread goes (share of its CPU, inclusive)"
$PERF report -i /tmp/dp.data --children --stdio -g none --percent-limit 1.5 2>/dev/null | grep -v "^#" | grep "%" | grep -E "App>::draw$|App>::draw_options$|Renderer::render$|TextRenderer::prepare|swash_image$|queue_write_texture$|queue_submit$|render_pass_end$|poll_single_device$|get_current_texture|::present$|shape_until_scroll|Scene|build_scene|push_|layout|measure_text|hypr::|surface_present|create_buffer_init|device_create_buffer|TileMap|mark_|Buffer::set_text|ShapeLine|content::|options::|panel::|i915_gem_execbuffer2_ioctl|__GI___ioctl$|__poll|ppoll|epoll_wait" | sed "s/  *\.waverunner-wra  */ /; s/\.waverunner-wrapped *//; s/libvulkan_intel_hasvk.so *//; s/libc.so.6 *//; s/\[kernel.kallsyms\] *//" | awk '{print $1, $3, $4, $5, $6, $7}' | cut -c1-130 | head -34
