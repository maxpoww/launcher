#!/usr/bin/env bash
# Regenerate crates/daemon/assets/pkg-popularity.tsv: how widely each nixpkgs
# package is USED, for the Install section's ranking (Max, 2026-10-30: the
# packages people use must never end up under "More").
#
# Two public counts of real machines, the higher share of the two:
#   - Debian's popularity contest (popcon.debian.org, by_inst): ~295 000
#     machines report which packages they have installed;
#   - Arch Linux's pkgstats (pkgstats.archlinux.de): ~32 000, and it knows the
#     newer desktop apps Debian does not package (ghostty, zed, obsidian…).
# Each nixpkgs attr is matched to their names by its own attr and pname, a
# kdePackages attr by its tail, and the names that differ by aliases.tsv.
#
# Vendored (as the emoji tables are): fetched once here, read by the dock from
# its binary — the dock itself goes on no network.
#
#   tools/pkg-popularity/gen-popularity.sh [index.tsv]
#     index: a waverunner nixpkgs index (default: the dev box's cache)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
INDEX="${1:-$HOME/.cache/waverunner/nixpkgs-index.tsv}"
OUT="$REPO/crates/daemon/assets/pkg-popularity.tsv"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo ">> fetching Debian popcon" >&2
curl -fsSL https://popcon.debian.org/by_inst.gz -o "$tmp/by_inst.gz"
echo ">> fetching Arch pkgstats" >&2
for offset in 0 10000 20000 30000 40000; do
  curl -fsSL "https://pkgstats.archlinux.de/api/packages?limit=10000&offset=$offset" -o "$tmp/pkgstats-$offset.json"
done
python3 -I "$HERE/gen_popularity.py" "$tmp/by_inst.gz" "$tmp" "$HERE/aliases.tsv" "$INDEX" "$OUT"
