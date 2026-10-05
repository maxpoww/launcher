#!/bin/bash
# pushroot.sh <name> <store path> [host]: copy a closure to a lab machine and root it there (so its nightly
# garbage collection keeps it). Roots live in /nix/var/nix/gcroots/lab/ — remove that directory when done.
NIX=/nix/store/qrl87rs5m0rd4wf3qgb4yq13x2mnl8qh-nix-2.34.8/bin/nix
export NIX_SSHOPTS="-o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR"
H=${3:-192.168.1.99}
$NIX --extra-experimental-features nix-command copy --no-check-sigs --to ssh://root@$H "$2" >/dev/null 2>&1 || { echo "copy of $2 failed"; exit 1; }
ssh $NIX_SSHOPTS root@$H "mkdir -p /nix/var/nix/gcroots/lab && ln -sfn $2 /nix/var/nix/gcroots/lab/$1 && echo rooted $1" < /dev/null
