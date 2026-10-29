#!/usr/bin/env bash
# Regenerate crates/daemon/src/emoji_words.rs: the emoji picker's SPANISH search words, from
# Unicode CLDR's annotations (es + es-419, and the derived ones: flags, skin tones, families),
# for the emoji of gemoji's table (`gen-emoji.sh`). CLDR is under the Unicode License v3; the
# generated table carries that provenance in its header.
#
#   tools/gen-emoji/gen-emoji-words.sh [cldr-json tag] [gemoji ref]   # defaults below
set -euo pipefail
CLDR="${1:-47.0.0}"
REF="${2:-v4.1.0}"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$REPO/crates/daemon/src/emoji_words.rs"
RAW="https://raw.githubusercontent.com/unicode-org/cldr-json/$CLDR/cldr-json"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo ">> fetching gemoji $REF and CLDR $CLDR (es, es-419)" >&2
curl -fsSL "https://raw.githubusercontent.com/github/gemoji/$REF/db/emoji.json" -o "$tmp/gem.json"
curl -fsSL "$RAW/cldr-annotations-full/annotations/es/annotations.json" -o "$tmp/es.json"
curl -fsSL "$RAW/cldr-annotations-derived-full/annotationsDerived/es/annotations.json" -o "$tmp/esd.json"
curl -fsSL "$RAW/cldr-annotations-full/annotations/es-419/annotations.json" -o "$tmp/l.json"
curl -fsSL "$RAW/cldr-annotations-derived-full/annotationsDerived/es-419/annotations.json" -o "$tmp/ld.json"
jq -n -r --arg cldr "$CLDR" --slurpfile gem "$tmp/gem.json" --slurpfile es "$tmp/es.json" --slurpfile esd "$tmp/esd.json" \
  --slurpfile l "$tmp/l.json" --slurpfile ld "$tmp/ld.json" -f "$(dirname "$0")/gen-emoji-words.jq" > "$OUT"
command -v rustfmt >/dev/null && rustfmt --edition 2021 "$OUT"
echo ">> wrote $OUT ($(grep -c '^    ("' "$OUT") emoji)" >&2
