#!/bin/bash
# wlrun.sh <dock store path|old> <tag> [workloads]: swap the dock on the Acer, run workloads, fetch end-state screenshots
# DOCKENV="A=1 B=2" adds environment to the dock for this run.
S=/tmp/claude-1000/-home-max/008ddfd0-dfcd-49b8-8260-3830056a0d8b/scratchpad; H=${WLHOST:-192.168.1.99}
O5="-o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ServerAliveInterval=15 -o ServerAliveCountMax=8"
dock=$1; tag=$2; shift 2
ssh $O5 root@$H "bash -s $dock 40 $DOCKENV" < $S/swapdock.sh | tail -2
ssh $O5 root@$H "bash -s $tag $*" < $S/work.sh 2>&1 | grep -v Terminated | tee $S/work-$tag.txt
mkdir -p $S/wl; scp -q $O5 "root@$H:/tmp/wl-$tag-*.png" $S/wl/
