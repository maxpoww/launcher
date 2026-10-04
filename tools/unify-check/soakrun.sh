#!/bin/bash
S=/tmp/claude-1000/-home-max/008ddfd0-dfcd-49b8-8260-3830056a0d8b/scratchpad
O5="-o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR"
ssh $O5 root@192.168.1.99 "bash -s $1" < $S/soak.sh; mkdir -p $S/soak; scp -q $O5 "root@192.168.1.99:/tmp/soak-$1-*.png" $S/soak/
M=/nix/store/1zns14x0dv9krcnbznbxkqlj28jxmdcp-imagemagick-7.1.2-31/bin/magick
cd $S/soak && $M soak-$1-notif.png soak-$1-clip.png soak-$1-audio30.png soak-$1-audio60.png -resize 800x +append r1.png && $M soak-$1-audio-after.png soak-$1-stage.png soak-$1-stage-off.png soak-$1-end.png -resize 800x +append r2.png && $M r1.png r2.png -append sheet-$1.png
