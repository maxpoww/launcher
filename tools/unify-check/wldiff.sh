#!/bin/bash
# wldiff.sh <tagA> <tagB>: pixel-diff the end-of-workload screenshots
S=/tmp/claude-1000/-home-max/008ddfd0-dfcd-49b8-8260-3830056a0d8b/scratchpad; M=/nix/store/1zns14x0dv9krcnbznbxkqlj28jxmdcp-imagemagick-7.1.2-31/bin/magick
for f in $S/wl/wl-$1-*.png; do w=$(basename $f .png | sed "s/wl-$1-//"); g=$S/wl/wl-$2-$w.png; [ -f $g ] || continue
  $M compare -fuzz 3% -compose src -highlight-color red -lowlight-color none $f $g $S/wl/m.png 2>/dev/null
  echo "$w: $($M compare -metric AE -fuzz 3% $f $g null: 2>&1 | awk '{print $1}') px; region $($M $S/wl/m.png -alpha extract -trim -format '%wx%h%X%Y' info: 2>/dev/null)"; done
