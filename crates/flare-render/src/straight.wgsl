// Un-premultiplies the painted grid for Slint, which reads an imported wgpu texture as STRAIGHT
// alpha (docs/material.md E3). A fullscreen triangle; each colour is divided by its alpha.

@group(0) @binding(0) var src: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

@fragment
fn fs_straight(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = textureLoad(src, vec2<i32>(floor(pos.xy)), 0);
    if p.a <= 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    return vec4<f32>(min(p.rgb / p.a, vec3<f32>(1.0)), p.a);
}
