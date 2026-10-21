// Two instanced-quad pipelines: solid rects and atlas glyphs. All in encoded (sRGB byte) space.

struct Globals {
    viewport: vec2<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas: texture_2d<f32>;

fn corner(vi: u32) -> vec2<f32> {
    var c = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    return c[vi];
}

fn to_clip(px: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / globals.viewport.x * 2.0 - 1.0, 1.0 - px.y / globals.viewport.y * 2.0, 0.0, 1.0);
}

struct RectOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
}

@vertex
fn vs_rect(
    @builtin(vertex_index) vi: u32,
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
) -> RectOut {
    var out: RectOut;
    out.pos = to_clip(rect.xy + corner(vi) * rect.zw);
    out.color = color;
    return out;
}

@fragment
fn fs_rect(in: RectOut) -> @location(0) vec4<f32> {
    return in.color;
}

struct GlyphOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) dst: vec4<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) uv: vec2<f32>,
    @location(3) @interpolate(flat) is_color: f32,
}

@vertex
fn vs_glyph(
    @builtin(vertex_index) vi: u32,
    @location(0) dst: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) is_color: f32,
) -> GlyphOut {
    var out: GlyphOut;
    out.pos = to_clip(dst.xy + corner(vi) * dst.zw);
    out.dst = dst;
    out.color = color;
    out.uv = uv;
    out.is_color = is_color;
    return out;
}

@fragment
fn fs_glyph(in: GlyphOut) -> @location(0) vec4<f32> {
    let ofs = vec2<i32>(floor(in.pos.xy - in.dst.xy));
    let texel = textureLoad(atlas, vec2<i32>(in.uv) + ofs, 0);
    if in.is_color > 0.5 {
        return texel;
    }
    return vec4<f32>(in.color.rgb * texel.a, texel.a);
}
