#!/bin/bash
S=/tmp/claude-1000/-home-max/008ddfd0-dfcd-49b8-8260-3830056a0d8b/scratchpad
O5="-o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR"
ssh $O5 root@192.168.1.99 "bash -s $1" < $S/vis.sh; mkdir -p $S/vis; scp -q $O5 "root@192.168.1.99:/tmp/vis-$1-*.png" $S/vis/
