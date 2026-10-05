#!/bin/bash
# kmscmp2.sh <tagA> <tagB>: the real screen in the same still states under two builds/settings (the "-a" grabs of kms.sh)
S=/tmp/claude-1000/-home-max/008ddfd0-dfcd-49b8-8260-3830056a0d8b/scratchpad; M=/nix/store/1zns14x0dv9krcnbznbxkqlj28jxmdcp-imagemagick-7.1.2-31/bin/magick
cd $S/kms
for a in d-$1-*-a.png; do sc=$(basename $a -a.png | sed "s/d-$1-//"); b=d-$2-$sc-a.png; [ -f $b ] || continue
  n=$($M compare -metric AE -fuzz ${FUZZ:-0}% $a $b null: 2>&1 | awk '{print $1}')
  $M compare -fuzz ${FUZZ:-0}% -compose src -highlight-color red -lowlight-color none $a $b m.png 2>/dev/null
  echo "$sc: $n px differ (fuzz ${FUZZ:-0}%); region $($M m.png -alpha extract -trim -format '%wx%h%X%Y' info: 2>/dev/null); worst $($M compare -metric PAE $a $b null: 2>&1 | sed 's/.*(\(.*\))/\1/')"
done
