//! The 3D model viewer (issue #36): Flare's own page in a tab's panel that shows one `.glb`, `.gltf` or `.stl` file, for
//! the agent's `show_model` and a plugin's `view.model`. No COM and no Slint.
//!
//! The page is a mini-app Flare writes itself (`app::write_app`), like the audio player: its policy is the first thing
//! in it. The only script is the bundled viewer beside the page (`assets/three/viewer.js`, built once from
//! `tools/model-viewer/`), and the only thing it may fetch is the model beside the page, so the model's bytes never pass
//! through the page's source. No network, no eval, no WASM and no worker. A model too large or a file that is not a
//! model shows a message in the panel instead of a blank one; the page's `window.__flareModel` says how it settled.

use std::path::Path;

/// The largest model file the viewer takes (bytes): it is copied beside the page.
pub const MODEL_MAX: u64 = 64 * 1024 * 1024;
/// The model's name in the page's folder, before its extension.
pub const MODEL_BASE: &str = "model";
/// The viewer's file name in the page's folder.
pub const VIEWER_FILE: &str = "viewer.js";
/// The panel's header when the caller gave no title.
pub const DEFAULT_TITLE: &str = "3D model";
/// The files the viewer shows, by extension: a new format is one row here and one in `tools/model-viewer/src/viewer.js`.
pub const MODEL_EXTS: [&str; 3] = ["glb", "gltf", "stl"];

/// The extension of a model file the viewer shows, lower case; None for anything else. Both names of a resolved file
/// must say so (as typed and as the file system names it), so a link cannot change what is shown.
pub fn model_ext(typed: &str, real: &str) -> Option<&'static str> {
    let of = |p: &str| {
        Path::new(p)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
    };
    let (a, b) = (of(typed)?, of(real)?);
    if a != b {
        return None;
    }
    MODEL_EXTS.iter().copied().find(|e| *e == a)
}

/// The model's file name in the page's folder.
pub fn model_name(ext: &str) -> String {
    format!("{MODEL_BASE}.{ext}")
}

/// The page's policy: its own viewer script, and the model beside it as the only thing it may fetch. Pictures inside a
/// model reach the viewer as `blob:` or `data:` images.
pub fn csp() -> String {
    "default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; \
     worker-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'"
        .to_string()
}

/// Text as it may sit in HTML (an attribute or an element).
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// The viewer page for a model of extension `ext`, named `title`.
pub fn page(ext: &str, title: &str) -> String {
    let (file, ext, title) = (esc(&model_name(ext)), esc(ext), esc(title));
    let policy = csp();
    format!(
        r#"<!doctype html><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="{policy}"><meta http-equiv="x-dns-prefetch-control" content="off">
<meta name="color-scheme" content="light dark"><title>{title}</title>
<style>
html,body{{margin:0;height:100%;overflow:hidden;font:12px "Segoe UI Variable Text","Segoe UI",sans-serif;color:CanvasText;background:Canvas}}
canvas{{position:absolute;inset:0;width:100%;height:100%;display:block;outline:none}}
#bar{{position:absolute;top:8px;left:8px;display:flex;gap:6px}}
button{{font:inherit;color:inherit;background:Canvas;border:1px solid color-mix(in srgb,CanvasText 30%,transparent);border-radius:4px;padding:3px 9px;cursor:pointer}}
button[aria-pressed="true"]{{background:color-mix(in srgb,CanvasText 18%,Canvas)}}
#hint{{position:absolute;left:10px;right:10px;bottom:8px;opacity:.65;pointer-events:none;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}}
#msg{{position:absolute;inset:0;display:flex;align-items:center;justify-content:center;margin:0;padding:24px;text-align:center;font-size:14px;background:Canvas}}
[hidden]{{display:none!important}}
</style>
<body data-file="{file}" data-ext="{ext}">
<canvas id="view" tabindex="0" aria-label="{title}"></canvas>
<div id="bar"><button id="wire" type="button" hidden>Wireframe</button><button id="grid" type="button" hidden>Grid</button></div>
<p id="hint" hidden>Drag to orbit, scroll to zoom, right-drag to pan, double-click to reset</p>
<p id="msg" role="status">Loading the model...</p>
<script src="{VIEWER_FILE}"></script>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_file_is_known_by_both_its_names() {
        assert_eq!(model_ext("C:/a/b.GLB", "C:/a/b.glb"), Some("glb"));
        assert_eq!(model_ext("x.gltf", "x.gltf"), Some("gltf"));
        assert_eq!(model_ext("x.stl", "x.STL"), Some("stl"));
        assert_eq!(
            model_ext("x.glb", "x.exe"),
            None,
            "a link that changes kind"
        );
        assert_eq!(
            model_ext("x.stl", "x.glb"),
            None,
            "a link to another format"
        );
        assert_eq!(model_ext("x.obj", "x.obj"), None);
        assert_eq!(model_ext("noext", "noext"), None);
        assert_eq!(model_name("stl"), "model.stl");
    }

    #[test]
    fn the_policy_comes_first_and_lets_in_only_the_viewer_and_its_model() {
        let p = page("glb", "Part");
        assert!(p.starts_with(
            "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\""
        ));
        assert!(p.contains(&csp()));
        for needed in [
            "default-src 'none'",
            "script-src 'self';",
            "connect-src 'self';",
            "worker-src 'none'",
            "frame-src 'none'",
            "object-src 'none'",
            "base-uri 'none'",
            "form-action 'none'",
        ] {
            assert!(csp().contains(needed), "{needed}");
        }
        assert!(
            !csp().contains("unsafe-eval") && !csp().contains("wasm"),
            "no eval and no WASM"
        );
        assert!(p.contains("<script src=\"viewer.js\"></script>"));
        assert!(
            p.matches("<script").count() == 1,
            "no inline script, script-src would refuse it"
        );
        assert!(p.contains("data-file=\"model.glb\" data-ext=\"glb\""));
    }

    #[test]
    fn the_title_cannot_break_out_of_the_page() {
        let p = page("stl", "\"><script>x</script><b onmouseover=x>");
        assert!(!p.contains("<script>x"));
        assert!(!p.contains("<b onmouseover"));
        assert!(p.contains("&lt;b onmouseover=x&gt;"));
    }
}
