---
name: materials
description: Work on waverunner's rendering — wgpu pipelines, WGSL shaders, the frosted-glass box backdrop, rounded-rect SDFs, blur, neumorphic/edge shadows, the icon atlas, or the scene-composition order. Use whenever a change touches renderer.rs or crates/daemon/src/shaders/*.wgsl, or the look/material of a surface (glass, shadow, corner, blur, tint).
---

# materials (wgpu / WGSL)

The renderer is hand-rolled wgpu 24 with WGSL shaders. There is no material
framework — each look is a small pipeline. Know the composition order before
touching it.

## Scene composition (draw order matters — pieces overwrite, they don't all blend)

1. **Base scene → offscreen `scene_tex`** — grids, icons, rects rendered sharp.
2. **Blur** (`blur.wgsl`, separable Gaussian, two passes) → a blurred copy.
3. **Box backdrop** (`box_backdrop.wgsl`) — for an open box: an *erase* pass
   (`fs_erase`, blend `Zero, OneMinusSrcAlpha`) clears the box region, then a
   *fill* pass composites the **premultiplied** blurred scene back in. Net =
   `mix(base, blurred, coverage)`, and translucency is preserved (a stack
   floating over wallpaper stays see-through — never forced opaque/black).
4. **Panel + members** — the glass rect, then rows/pills/labels/thumbnails.

## Shader files

- `rounded_rect.wgsl` — the SDF everything's coverage comes from (`box_coverage`
  in `box_backdrop.wgsl` matches it; keep them in sync).
- `blur.wgsl` — separable Gaussian; runs twice (H then V).
- `box_backdrop.wgsl` — frosted glass (erase + fill, premultiplied).
- `edge_shadow.wgsl` — soft edge/drop shadow.
- `icon.wgsl` — instanced textured quads over a texture-array atlas.
- `blit.wgsl` — copy `scene_tex` to the swapchain.

## Gotchas

- **Premultiplied alpha** throughout the blurred path — don't un-premultiply or
  the glass goes wrong (halos, wrong translucency). See the `box_backdrop.wgsl`
  header.
- **Radius clamp**: SDF radius is `min(radius, min(half.x, half.y))`; a short
  rect (mid-animation) renders as a stadium, not a bug.
- **Instance buffers** are clipped by grid scissor, not per-instance. `RectInst`
  has no clip field — mask by clamping the rect (see the clipboard detail band).
- Neumorphic pills are a soft `ShadowInst` (`push_neumorph`) + a wash `RectInst`
  + a glyph `Label`; the shadow and rect aren't clipped, only the label is.

## Validating a shader change

WGSL is **not** checked by `cargo build` — wgpu/naga validates it when the
pipeline is created, i.e. at **daemon startup**. So:

1. `verify-ui` (build + restart). If the daemon doesn't come back up, the shader
   failed validation.
2. Read `/tmp/waverunner-verify/daemon.log` — naga parse/validation errors and
   pipeline-creation panics land there with line/column.
3. If it came up, screenshot the affected surface (`--reveal clip|notif`, tight
   `--geom` crop) and look — a shader bug that *validates* still renders wrong
   (halo, black box, wrong corner), which only the eye catches.

(For true offline validation, `naga-cli` could be added to the flake devShell —
`buildInputs = [ ... naga-cli ]` — then `naga file.wgsl`. Ask before editing the
flake; a broken flake blocks all builds.)
