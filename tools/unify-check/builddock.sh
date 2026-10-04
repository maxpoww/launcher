#!/bin/bash
# build the unify worktree's dock via Golem's flake and copy it to the Acer; prints the store path
set -e
NIX=/nix/store/qrl87rs5m0rd4wf3qgb4yq13x2mnl8qh-nix-2.34.8/bin/nix
T=$($NIX --extra-experimental-features nix-command --extra-experimental-features flakes build --no-link --print-out-paths --override-input waverunner git+file:///home/max/.cache/launcher-wt/unify "git+file:///home/max/.cache/golem-wt/iso#nixosConfigurations.golem-desktop-vm.config.system.build.toplevel" 2>/tmp/claude-1000/builddock.err | tail -1) || { tail -30 /tmp/claude-1000/builddock.err; exit 1; }
W=$($NIX --extra-experimental-features nix-command path-info -r $T | grep -E "waverunner-daemon-0.1.0$")
echo "$W" > /tmp/claude-1000/-home-max/008ddfd0-dfcd-49b8-8260-3830056a0d8b/scratchpad/newdock
export NIX_SSHOPTS="-o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR"
$NIX --extra-experimental-features nix-command copy --no-check-sigs --to ssh://root@${1:-192.168.1.99} $W >/dev/null 2>&1
echo "$W"
