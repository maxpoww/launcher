# third_party

## wgpu-hal (24.0.4, patched)

A copy of the `wgpu-hal` crate with one addition: a present can say which part
of the image changed. The workspace swaps it in through `[patch.crates-io]`
(root `Cargo.toml`); nothing else in wgpu is touched.

**Why.** wgpu has no API for present damage, so Mesa told the compositor
"the whole surface changed" on every frame. Our surfaces are much larger than
what they draw, and Hyprland redrew — and blurred again — everything behind
them per frame. See `crates/daemon/src/damage.rs` and
`docs/unify-audit-2026-10-04.md` (round 3).

**What changed** (`wgpu-hal.patch`, ~200 lines, four files):

- `src/lib.rs` — `present_damage::{set_next, clear}`: the renderer leaves the
  rectangles of the next present there.
- `src/vulkan/adapter.rs` — enables `VK_KHR_incremental_present` when the
  driver has it.
- `src/vulkan/mod.rs` — `Queue::present` chains `VkPresentRegionsKHR` with
  those rectangles.
- `src/gles/egl.rs` — `Surface::present` swaps with
  `eglSwapBuffersWithDamage{KHR,EXT}` when the display has it (rectangles
  flipped: EGL's origin is the bottom left).

Mesa turns both into `wl_surface.damage_buffer`. Without the extension, or
when nothing is set, a present is exactly stock.

**To update wgpu** (the copy must match the `wgpu-hal` version `wgpu` wants):

```sh
cp -r ~/.cargo/registry/src/*/wgpu-hal-<new>/ third_party/wgpu-hal
rm -f third_party/wgpu-hal/Cargo.lock third_party/wgpu-hal/.cargo-ok \
      third_party/wgpu-hal/.cargo_vcs_info.json
patch -p1 -d third_party < third_party/wgpu-hal.patch   # rebase the hunks if it rejects
cargo check -p waverunner-daemon
```

The crate's files keep their CRLF line endings; edit them with a tool that
preserves those, or the patch file grows to the size of the crate.

## glyphon (0.8.0, patched)

A copy of the `glyphon` crate (its library only: no examples, benches or
samples; their targets are taken out of `Cargo.toml`) with one change: a
COLOUR glyph fades with its words. The workspace swaps it in through
`[patch.crates-io]`.

**Why.** glyphon's shader drew a colour glyph (an emoji, from Noto Color Emoji)
straight from its atlas and never looked at the text's colour, alpha
included. The shell fades its words by that alpha — the box opening and
closing, a page of recents sliding away, a dim name — and an emoji stayed
whole while the words beside it faded (2026-10-29, once every emoji was
shaped in the colour font: `renderer::set_words`).

**What changed** (`glyphon.patch`, one hunk): `src/shader.wgsl` — `fs_main`
returns the colour glyph's own colour with its alpha times the text's.

**To update glyphon**: as wgpu-hal's above, with `glyphon-<new>` and
`glyphon.patch` (copy `src/`, the licences, README; take the `[[example]]` and
`[[bench]]` targets out of its `Cargo.toml`). The shader keeps CRLF endings.
