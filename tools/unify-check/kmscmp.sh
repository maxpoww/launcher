#!/bin/bash
# kmscmp.sh <tag>: fetch, de-tile and compare the real-screen grabs (a = after partial frames, b = after a full redraw)
S=/tmp/claude-1000/-home-max/008ddfd0-dfcd-49b8-8260-3830056a0d8b/scratchpad; M=/nix/store/1zns14x0dv9krcnbznbxkqlj28jxmdcp-imagemagick-7.1.2-31/bin/magick
O5="-o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR"
mkdir -p $S/kms; scp -q $O5 "root@${WLHOST:-192.168.1.99}:/tmp/kms-$1-*.png" $S/kms/
cd $S/kms
for a in kms-$1-*-a.png; do sc=$(basename $a -a.png | sed "s/kms-$1-//"); b=kms-$1-$sc-b.png; [ -f $b ] || continue
  for x in a b; do $M kms-$1-$sc-$x.png -depth 8 rgba:t.rgba; python3 $S/detile.py t.rgba t2.rgba 1366 768 >/dev/null; $M -size 1366x768 -depth 8 rgba:t2.rgba -alpha off d-$1-$sc-$x.png; done
  n=$($M compare -metric AE -fuzz ${FUZZ:-0}% d-$1-$sc-a.png d-$1-$sc-b.png diff-$1-$sc.png 2>&1 | awk '{print $1}')
  $M compare -fuzz ${FUZZ:-0}% -compose src -highlight-color red -lowlight-color none d-$1-$sc-a.png d-$1-$sc-b.png m.png 2>/dev/null
  echo "$sc: $n px differ; region $($M m.png -alpha extract -trim -format '%wx%h%X%Y' info: 2>/dev/null)"
done
