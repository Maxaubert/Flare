//! The 3D model viewer (issue #36): `model`.
//!
//! Starts the built release exe in its own world and profile and calls `view.model` over the control channel on small
//! models this file writes. The window is parked off the screen and never takes the foreground, like every scenario.
//!
//! It holds: a `.glb`, a binary and an ASCII `.stl` with zero normals (shaded, not one flat silhouette) and a `.gltf`
//! with its data inside all load through the page's policy and draw (pixels
//! other than the background, with the grid off); the wireframe and grid buttons; the title; a damaged file, a `.gltf`
//! that points at another file and a model over the triangle limit each show their message; the size cap and the
//! refusals. Reading a file needs no graphics, so the failures are checked either way.
//!
//! WebGL may not be there (a machine with no GPU and no software fallback). Then a good model shows the viewer's own
//! message, which is checked, and the drawing checks are reported as a `skip` line: a skip is never a pass.

use std::fs;
use std::path::Path;

use flare_ctl::Method;
use flare_e2e_proto::Request;
use serde_json::{json, Value};

use super::input_b::prompt;
use super::mini_app::{say, until_says};
use super::web::{channel_of, code_of, hello, launch_with, LOAD};
use crate::ctx::{App, Ctx};

/// A tiny pyramid: four corners and four faces, so every view of it covers pixels.
const CORNERS: [[f32; 3]; 4] = [
    [0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
];
const FACES: [[u16; 3]; 4] = [[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]];

/// The triangle limit the viewer holds (`TRIANGLES_MAX` in `tools/model-viewer/src/viewer.js`).
const TRIANGLES_MAX: usize = 5_000_000;

/// The STL pyramid: its point faces the viewer's opening camera, so several faces show. Lit faces then differ, and only
/// lighting makes them differ: the `CORNERS` pyramid shows that camera one face, which is flat whatever its normals are.
const STL_CORNERS: [[f32; 3]; 4] = [
    [1.0, 1.0, 1.0],
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
];
const STL_FACES: [[u16; 3]; 4] = [[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]];

/// The least a lit grey part measures on `window.__flareModelShade`: its mean brightness and the gap between its faces.
const MEAN_LIT: u64 = 40;
const SPREAD_LIT: u64 = 5;

fn corners_bytes() -> Vec<u8> {
    CORNERS
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect()
}

/// The glTF description of a mesh with `index_count` indices after the corners in one buffer.
fn gltf_json(index_count: usize, buffer: Value) -> Value {
    let corners = corners_bytes().len();
    json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1}]}],
        "buffers": [buffer],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": corners},
            {"buffer": 0, "byteOffset": corners, "byteLength": index_count * 2},
        ],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3",
             "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 1.0]},
            {"bufferView": 1, "componentType": 5123, "count": index_count, "type": "SCALAR"},
        ],
    })
}

/// A binary glTF of the pyramid, its faces repeated `repeat` times.
fn glb(repeat: usize) -> Vec<u8> {
    let bin = pyramid_bin(repeat);
    pack_glb(
        gltf_json(repeat * 12, json!({"byteLength": bin.len()})),
        bin,
    )
}

/// A one-pixel PNG, the picture inside `textured_glb`.
const PIXEL_PNG: [u8; 70] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xfc, 0xcf, 0xc0, 0x50,
    0x0f, 0x00, 0x04, 0x85, 0x01, 0x80, 0x84, 0xa9, 0x8c, 0x21, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// The pyramid with a picture inside the file: three's glTF loader reads such a picture through a `blob:` address.
fn textured_glb() -> Vec<u8> {
    let mut bin = pyramid_bin(1);
    let uv_at = bin.len();
    for v in [0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0] {
        bin.extend_from_slice(&v.to_le_bytes());
    }
    let image_at = bin.len();
    bin.extend_from_slice(&PIXEL_PNG);
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let mut d = gltf_json(12, json!({"byteLength": bin.len()}));
    d["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"] = json!(2);
    d["meshes"][0]["primitives"][0]["material"] = json!(0);
    d["materials"] = json!([{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}]);
    d["textures"] = json!([{"source": 0}]);
    d["images"] = json!([{"bufferView": 3, "mimeType": "image/png"}]);
    let views = d["bufferViews"].as_array_mut().unwrap();
    views.push(json!({"buffer": 0, "byteOffset": uv_at, "byteLength": 32}));
    views.push(json!({"buffer": 0, "byteOffset": image_at, "byteLength": PIXEL_PNG.len()}));
    let accessors = d["accessors"].as_array_mut().unwrap();
    accessors.push(json!({"bufferView": 2, "componentType": 5126, "count": 4, "type": "VEC2"}));
    pack_glb(d, bin)
}

/// The pyramid's corners then its faces repeated `repeat` times, 4-byte aligned.
fn pyramid_bin(repeat: usize) -> Vec<u8> {
    let mut bin = corners_bytes();
    for _ in 0..repeat {
        for i in FACES.iter().flatten() {
            bin.extend_from_slice(&i.to_le_bytes());
        }
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    bin
}

/// A glTF description and its binary chunk as one `.glb` file.
fn pack_glb(description: Value, bin: Vec<u8>) -> Vec<u8> {
    let mut text = description.to_string().into_bytes();
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    let total = 12 + 8 + text.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&text);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(ABC[(n >> 18) as usize & 63] as char);
        out.push(ABC[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            ABC[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            ABC[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// A `.gltf` of the pyramid with its data inside itself, as a `data:` address.
fn embedded_gltf() -> String {
    let mut bin = corners_bytes();
    for i in FACES.iter().flatten() {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    let uri = format!("data:application/octet-stream;base64,{}", base64(&bin));
    gltf_json(12, json!({"byteLength": bin.len(), "uri": uri})).to_string()
}

/// A `.gltf` of the pyramid that points at a `.bin` beside it.
fn external_gltf() -> String {
    gltf_json(12, json!({"byteLength": 60, "uri": "part.bin"})).to_string()
}

/// An ASCII STL of `STL_CORNERS`, every facet's normal left at zero as many exporters do.
fn ascii_stl() -> String {
    let mut out = String::from("solid pyramid\n");
    for face in STL_FACES {
        out.push_str(" facet normal 0 0 0\n  outer loop\n");
        for i in face {
            let [x, y, z] = STL_CORNERS[usize::from(i)];
            out.push_str(&format!("   vertex {x:.1} {y:.1} {z:.1}\n"));
        }
        out.push_str("  endloop\n endfacet\n");
    }
    out.push_str("endsolid pyramid\n");
    out
}

/// A binary STL of `STL_CORNERS`, every facet's normal left at zero as many exporters do.
fn stl() -> Vec<u8> {
    let mut out = vec![0u8; 80];
    out.extend_from_slice(&(STL_FACES.len() as u32).to_le_bytes());
    for face in STL_FACES {
        out.extend_from_slice(&[0u8; 12]);
        for i in face {
            for v in STL_CORNERS[usize::from(i)] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out.extend_from_slice(&[0u8; 2]);
    }
    out
}

/// Waits for the page's `window.__flareModel` and gives what it settled to.
fn settled(ctx: &mut Ctx, app: &mut App) -> Value {
    say(
        ctx,
        app,
        "window.__settled = null; window.__flareModel.then(function (r) { window.__settled = r; }); 'waiting'",
    );
    let (_, text) = until_says(
        ctx,
        "the model to settle",
        30_000,
        app,
        "JSON.stringify(window.__settled)",
        &|s| s.starts_with('{'),
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

fn shown(ctx: &mut Ctx, app: &mut App) -> String {
    say(ctx, app, "document.getElementById('msg').textContent")
}

/// Shows `path` and gives what the page settled to.
fn open(
    ctx: &mut Ctx,
    app: &mut App,
    c: &mut flare_ctl::Client<flare_ctl::PipeEnd>,
    path: &Path,
    what: &str,
) -> Value {
    let r = c.call(
        Method::ViewModel,
        json!({ "path": path.to_string_lossy(), "title": what }),
        LOAD,
    );
    ctx.ok(
        r.as_ref()
            .is_ok_and(|v| v["panel"]["kind"] == json!("app") && v["loaded"] == json!(true)),
        format!("model: {what}: view.model answers once the page has loaded ({r:?})"),
    );
    settled(ctx, app)
}

pub fn model(ctx: &mut Ctx) -> Result<(), String> {
    let root = ctx.ensure_world().root.join("models");
    fs::create_dir_all(&root).map_err(|e| format!("make the models folder: {e}"))?;
    let mut app = launch_with(ctx, &root, 96, &[])?;
    let r = model_on(ctx, &mut app, &root);
    let left = app.close();
    ctx.ok(left, "the app quits by itself");
    r
}

fn model_on(ctx: &mut Ctx, app: &mut App, root: &Path) -> Result<(), String> {
    let at = "model";
    app.session.call(&Request::Resize {
        w: 1000.0,
        h: 640.0,
    })?;
    prompt(app)?;
    let (pipe, _, token) = channel_of(app)?;
    let mut c = hello(&pipe, &token).map_err(|e| format!("hello: {e}"))?;
    let write = |name: &str, bytes: &[u8]| -> Result<std::path::PathBuf, String> {
        let p = root.join(name);
        fs::write(&p, bytes).map_err(|e| format!("write {name}: {e}"))?;
        Ok(p)
    };

    // ---- a good model of each format ----
    let good = [
        ("part.glb", glb(1)),
        ("part.stl", stl()),
        ("ascii.stl", ascii_stl().into_bytes()),
        ("embedded.gltf", embedded_gltf().into_bytes()),
    ];
    let mut webgl = true;
    for (name, bytes) in &good {
        let path = write(name, bytes)?;
        let v = open(ctx, app, &mut c, &path, name);
        if v["ok"] == json!(true) {
            ctx.ok(
                v["triangles"] == json!(4),
                format!("{at}: {name} has its four triangles ({v})"),
            );
            let bar = say(ctx, app, "String(document.getElementById('msg').hidden)");
            ctx.ok(bar == "true", format!("{at}: {name} leaves no message up"));
            if *name == "part.glb" {
                pixels_and_toggles(ctx, app);
            }
            if name.ends_with(".stl") {
                is_shaded(ctx, app, name);
            }
        } else if v["code"] == json!("webgl") {
            webgl = false;
            ctx.skip(&format!(
                "{at}: WebGL is NOT available in this run ({}), so {name} was not drawn",
                v["error"]
            ));
            let m = shown(ctx, app);
            ctx.ok(
                m.contains("WebGL is not available"),
                format!("{at}: without WebGL the panel says so ({m:?})"),
            );
        } else {
            ctx.ok(false, format!("{at}: {name} did not load ({v})"));
        }
    }

    // ---- a picture inside the file reaches the material through the page's policy (it needs no graphics) ----
    let path = write("textured.glb", &textured_glb())?;
    let v = open(ctx, app, &mut c, &path, "textured.glb");
    ctx.ok(
        v["textures"] == json!(1) || v["code"] == json!("webgl"),
        format!("{at}: the picture inside the file is loaded ({v})"),
    );

    // ---- files the viewer cannot show: a message each, whatever the graphics ----
    let bad = [
        (
            "damaged.glb",
            b"this is not a model".to_vec(),
            "corrupt",
            "could not be read",
        ),
        (
            "outside.gltf",
            external_gltf().into_bytes(),
            "external",
            "separate files",
        ),
        (
            "huge.glb",
            glb(TRIANGLES_MAX / 4 + 1),
            "big",
            "too many triangles",
        ),
    ];
    for (name, bytes, code, words) in &bad {
        let path = write(name, bytes)?;
        let v = open(ctx, app, &mut c, &path, name);
        ctx.ok(
            v["ok"] == json!(false) && v["code"] == json!(*code),
            format!("{at}: {name} settles as {code} ({v})"),
        );
        let m = shown(ctx, app);
        ctx.ok(
            m.contains(words),
            format!("{at}: {name} shows its message ({m:?})"),
        );
    }

    // ---- refusals ----
    let over = root.join("over.glb");
    fs::File::create(&over)
        .and_then(|f| f.set_len(64 * 1024 * 1024 + 1))
        .map_err(|e| format!("make the big file: {e}"))?;
    let exe = write("run.exe", b"MZ")?;
    for (params, want) in [
        (json!({ "path": over.to_string_lossy() }), "bad_params"),
        (json!({ "path": exe.to_string_lossy() }), "not_found"),
        (
            json!({ "path": root.join("missing.glb").to_string_lossy() }),
            "not_found",
        ),
        (json!({}), "bad_params"),
    ] {
        let r = c.call(Method::ViewModel, params.clone(), LOAD);
        ctx.ok(
            code_of(&r) == want,
            format!("{at}: {params} is {want} ({})", code_of(&r)),
        );
    }
    if !webgl {
        ctx.skip("model: the drawing, wireframe and grid checks did not run without WebGL");
    }
    Ok(())
}

/// An STL whose facets carry no normal is still lit: a bright part whose faces differ, not one flat silhouette (its colour
/// depends on the GPU: zero normals measured a single grey here, and are black on others).
fn is_shaded(ctx: &mut Ctx, app: &mut App, name: &str) {
    say(ctx, app, "document.getElementById('grid').click(); 'x'");
    let text = say(ctx, app, "JSON.stringify(window.__flareModelShade())");
    let shade: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let (mean, spread) = (shade["mean"].as_u64(), shade["spread"].as_u64());
    ctx.ok(
        mean.is_some_and(|m| m >= MEAN_LIT) && spread.is_some_and(|s| s >= SPREAD_LIT),
        format!("model: {name} is shaded, not a flat silhouette ({text})"),
    );
}

/// The model is on the canvas: with the grid off the only thing that is not background is the model.
fn pixels_and_toggles(ctx: &mut Ctx, app: &mut App) {
    let at = "model";
    let pressed = |ctx: &mut Ctx, app: &mut App, id: &str| {
        say(
            ctx,
            app,
            &format!("document.getElementById('{id}').getAttribute('aria-pressed')"),
        )
    };
    let (grid, wire) = (pressed(ctx, app, "grid"), pressed(ctx, app, "wire"));
    ctx.ok(
        grid == "true" && wire == "false",
        format!("{at}: the grid starts on and the wireframe off"),
    );
    say(ctx, app, "document.getElementById('grid').click(); 'x'");
    let grid = pressed(ctx, app, "grid");
    ctx.ok(
        grid == "false",
        format!("{at}: the grid button turns the grid off"),
    );
    let solid: u64 = say(ctx, app, "String(window.__flareModelPixels())")
        .parse()
        .unwrap_or(0);
    ctx.ok(
        solid > 500,
        format!("{at}: the model is drawn ({solid} pixels that are not background)"),
    );
    say(ctx, app, "document.getElementById('wire').click(); 'x'");
    let wire: u64 = say(ctx, app, "String(window.__flareModelPixels())")
        .parse()
        .unwrap_or(0);
    let on = pressed(ctx, app, "wire");
    ctx.ok(
        on == "true" && wire > 0 && wire < solid,
        format!("{at}: the wireframe draws less than the solid ({wire} of {solid} pixels)"),
    );
    let t = say(ctx, app, "document.title");
    ctx.ok(
        t == "part.glb",
        format!("{at}: the page has the title ({t:?})"),
    );
}
