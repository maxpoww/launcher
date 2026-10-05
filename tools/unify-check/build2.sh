#!/bin/bash
# build the round-3 worktree's dock via Golem's flake, root it here and on the Acer; prints the store path
set -e
S=/tmp/claude-1000/-home-max/008ddfd0-dfcd-49b8-8260-3830056a0d8b/scratchpad
NIX=/nix/store/qrl87rs5m0rd4wf3qgb4yq13x2mnl8qh-nix-2.34.8/bin/nix
R=$HOME/.cache/unify3-roots; mkdir -p $R
GOLEM=${GOLEM_WT:-/home/max/.cache/golem-wt/damage}
T=$($NIX --extra-experimental-features nix-command --extra-experimental-features flakes build --out-link $R/toplevel-vm --print-out-paths --override-input waverunner git+file:///home/max/.cache/launcher-wt/round2 "git+file://$GOLEM#nixosConfigurations.golem-desktop-vm.config.system.build.toplevel" 2>/tmp/claude-1000/builddock.err | tail -1) || { tail -30 /tmp/claude-1000/builddock.err; exit 1; }
W=$($NIX --extra-experimental-features nix-command path-info -r $T | grep -E "waverunner-daemon-0.1.0$")
C=$($NIX --extra-experimental-features nix-command path-info -r $T | grep -E "waverunner-ctl-0.1.0$" | head -1)
H=$($NIX --extra-experimental-features nix-command path-info -r $T | grep -E -- "-hyprland-0.55.4$" | head -1)
echo "$W" > $S/newdock; echo "$C" > $S/ctlpath; echo "$H" > $S/newhypr
ln -sfn $W $R/dock; ln -sfn $H $R/hyprland; [ -n "$C" ] && ln -sfn $C $R/ctl
if [ -z "$NOPUSH" ]; then
  $S/pushroot.sh dock $W ${1:-192.168.1.99} >/dev/null
  [ -n "$C" ] && $S/pushroot.sh ctl $C ${1:-192.168.1.99} >/dev/null
fi
echo "$W"
