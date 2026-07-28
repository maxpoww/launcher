# waverunner declarative installs — NixOS integration

waverunner installs/uninstalls packages declaratively. The dock only ever
edits one user-owned data file; a small privileged helper turns that into a
`nixos-rebuild switch`. This directory holds the helper module.

## Model

```
~/.config/waverunner/packages.list   user-writable DATA (one nixpkgs attr/line)
        │  waverunner appends / removes a line
        ▼
  systemd .path watch → waverunner-apply.service (root, oneshot)
        │  validate as data (strict ^[a-zA-Z0-9][a-zA-Z0-9._-]*$; never imported)
        ▼
/etc/nixos/waverunner-packages.nix   ROOT-owned, generated, imported by home.nix
        ▼
  nixos-rebuild switch   → result written to ~/.config/waverunner/apply-status.json
```

The privilege boundary is the file: the helper parses the list as **data**
and generates the Nix itself, so a user-writable file can never inject an
expression into a root rebuild. A failed rebuild is atomic (never activates)
and the helper restores the last-good generated file.

## Install (NixOS-module home-manager, channel-based)

1. Copy the module in:
   ```sh
   sudo cp nixos/waverunner-apply.nix /etc/nixos/waverunner-apply.nix
   ```
2. Import it from `configuration.nix`:
   ```nix
   imports = [ ./waverunner-apply.nix /* … */ ];
   ```
3. Create the generated file `home.nix` imports (starts empty; the helper
   regenerates it), and import it:
   ```sh
   printf '{ pkgs, ... }:\n{ home.packages = with pkgs; [ ]; }\n' \
     | sudo tee /etc/nixos/waverunner-packages.nix
   ```
   ```nix
   # in home.nix
   imports = [ ./waverunner-packages.nix /* … */ ];
   ```
4. `sudo nixos-rebuild switch`.

Adjust `user`/paths at the top of `waverunner-apply.nix` for a different
username. GC, store-optimise and `allowUnfree` are assumed already set in
your config; the module only adds `system.autoUpgrade` (weekly, no reboot)
plus the apply path+service.

## Smoke test

```sh
echo "# ping" >> ~/.config/waverunner/packages.list
sleep 3 && cat ~/.config/waverunner/apply-status.json   # {"phase":"done","ok":true,…}
```
