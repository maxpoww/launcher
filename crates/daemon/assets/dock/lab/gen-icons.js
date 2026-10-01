const fs = require("fs");
const src = fs.readFileSync("icon-lab.html", "utf8");
// Run from this folder: node gen-icons.js ..  (pulls the shape code out of
// icon-lab.html, the mockup at https://claude.ai/artifact/PrgfnPYLVCeAV15gTqF8aC).
const pick = (start, end) => { const i = src.indexOf(start); const j = src.indexOf(end, i); return src.slice(i, j); };
eval(pick("function gearPath", "const svgURI").replace("const GLYPH", "var GLYPH"));
const S = { apps: "disc", bin: "round", gear: "gear8" };
// Max's settings (2026-10-01): Cast finish, gold 0, shine 0, brightness 1.15,
// every figure at 0.90 of the plate, opacity 0.60 (raised to 0.80: "brighter").
const hsl = (l) => { const v = Math.round(Math.min(99, l * 1.15) / 100 * 255); const h = v.toString(16).padStart(2, "0"); return "#" + h + h + h; };
// "Whiter" (Max, 2026-10-01): every stop pulled this far toward white.
const WHITEN = 0.8;
const white = (hex) => { const v = parseInt(hex.slice(1, 3), 16); const w = Math.round(v + (255 - v) * WHITEN).toString(16).padStart(2, "0"); return "#" + w + w + w; };
const hi = white(hsl(94)), mid = white(hsl(78)), lo = white(hsl(58)), deep = white(hsl(44));
function svg(inner) {
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="256" height="256">
  <!-- Golem dock figure: cast silver, from the metal icon lab (Max, 2026-10-01).
       Drawn at 0.90 of the plate (the dock draws the plate itself). -->
  <defs>
    <linearGradient id="cast" x1="0.44" y1="0" x2="0.56" y2="1">
      <stop offset="0" stop-color="${hi}"/><stop offset="0.38" stop-color="${mid}"/>
      <stop offset="0.72" stop-color="${lo}"/><stop offset="1" stop-color="${deep}"/>
    </linearGradient>
    <linearGradient id="gloss" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#fff" stop-opacity="0.22"/><stop offset="0.55" stop-color="#fff" stop-opacity="0"/>
    </linearGradient>
    <filter id="bevel" x="-20%" y="-20%" width="140%" height="140%">
      <feDropShadow dx="0" dy="-1.8" stdDeviation="0" flood-color="#fff" flood-opacity="0.5"/>
      <feDropShadow dx="0" dy="4" stdDeviation="1.8" flood-color="#000" flood-opacity="0.5"/>
    </filter>
  </defs>
  <g opacity="0.8">
    <g transform="translate(5 5) scale(0.9)">
      <g filter="url(#bevel)" fill="url(#cast)">${inner}</g>
      <g fill="url(#gloss)">${inner}</g>
    </g>
  </g>
</svg>
`;
}
const out = process.argv[2];
fs.writeFileSync(out + "/apps.svg", svg(GLYPH.apps(S.apps)));
fs.writeFileSync(out + "/bin.svg", svg(GLYPH.bin(S.bin, false)));
// The bin in two pieces so the dock can raise the lid under a drag: the
// can body, and the lid (knob + rim) alone, on the same canvas.
{
  const full = GLYPH.bin(S.bin, false);
  const parts = full.match(/<(path|rect)[^>]*\/>/g);
  const lid = parts.filter((p) => !/fill-rule="evenodd"/.test(p)).join("");
  const body = parts.filter((p) => /fill-rule="evenodd"/.test(p)).join("");
  fs.writeFileSync(out + "/bin-body.svg", svg(body));
  fs.writeFileSync(out + "/bin-lid.svg", svg(lid));
}
fs.writeFileSync(out + "/control.svg", svg(GLYPH.gear(S.gear)));
console.log("wrote", hi, mid, lo, deep);
