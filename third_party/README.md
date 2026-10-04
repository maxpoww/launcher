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

**What changed** (`wgpu-hal.patch`, ~100 lines, three files):

- `src/lib.rs` — `present_damage::{set_next, clear}`: the renderer leaves the
  rectangles of the next present there.
- `src/vulkan/adapter.rs` — enables `VK_KHR_incremental_present` when the
  driver has it.
- `src/vulkan/mod.rs` — `Queue::present` chains `VkPresentRegionsKHR` with
  those rectangles (Mesa turns them into `wl_surface.damage_buffer`).

Without the extension, or when nothing is set, a present is exactly stock.
The GL backend is untouched (it still presents full damage).

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
