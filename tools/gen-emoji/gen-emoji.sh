#!/usr/bin/env bash
# Regenerate crates/daemon/src/emoji_table.rs from github/gemoji.
#
# The emoji picker's data: the character, its CLDR name, gemoji's shortcode
# aliases + tags (the search words — "happy" is a tag of 😀, whose name is only
# "grinning face"), and its category. gemoji is MIT-licensed; the generated
# table carries that provenance in its header.
#
# Vendored as generated Rust rather than built by the flake like the
# dictionaries: it is ~150 KB, and the picker should not have a data file to
# find, a load to wait for, or a "not installed" state.
#
#   tools/gen-emoji/gen-emoji.sh [git-ref]     # default: the pinned tag below
set -euo pipefail
REF="${1:-v4.1.0}"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$REPO/crates/daemon/src/emoji_table.rs"
URL="https://raw.githubusercontent.com/github/gemoji/$REF/db/emoji.json"

echo ">> fetching gemoji $REF" >&2
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
curl -fsSL "$URL" -o "$tmp"
jq -r -f "$(dirname "$0")/gen-emoji.jq" "$tmp" > "$OUT"
echo ">> wrote $OUT ($(grep -c 'EmojiDef {' "$OUT") emoji)" >&2
