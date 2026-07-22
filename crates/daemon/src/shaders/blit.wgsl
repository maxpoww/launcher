// Fullscreen blit: draws one big triangle and samples a source texture
// 1:1 onto the target. Used to copy the offscreen scene texture back to
// the swapchain (and, later, to composite the blurred box backdrop). The
// scene is authored in premultiplied alpha; a plain sample + replace-blend
// preserves those premultiplied values exactly.

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // Oversized triangle covering the viewport; uv in [0,1] over the screen.
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    var out: VsOut;
    out.uv = vec2<f32>(x, y);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src, src_sampler, in.uv);
}
