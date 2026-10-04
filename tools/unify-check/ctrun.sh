#!/bin/bash
# ctrun.sh <tag>: run the colour test on the Acer with whatever dock runs, print bar vs window colours
S=/tmp/claude-1000/-home-max/008ddfd0-dfcd-49b8-8260-3830056a0d8b/scratchpad
O5="-o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR"
ssh $O5 root@192.168.1.99 "bash -s $1" < $S/colortest.sh >/dev/null; mkdir -p $S/ct; scp -q $O5 "root@192.168.1.99:/tmp/ct-$1-*.png" $S/ct/
M=/nix/store/1zns14x0dv9krcnbznbxkqlj28jxmdcp-imagemagick-7.1.2-31/bin/magick
for f in a b1 b2 c1 c2 z; do echo "$1 $f: bar@(300,12)=$($M $S/ct/ct-$1-$f.png -format '%[pixel:p{300,12}]' info:)  window@(300,80)=$($M $S/ct/ct-$1-$f.png -format '%[pixel:p{300,80}]' info:)"; done
