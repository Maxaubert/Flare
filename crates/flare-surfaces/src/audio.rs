//! The audio player (plugins phase 6, design 5.6 and 8): Flare's own page in a tab's panel that plays one sound file
//! or one public `https` address, for the agent's `show_audio` and a plugin's `view.audio`. No COM and no Slint.
//!
//! The page is a mini-app Flare writes itself (`app::write_app`): its policy is the first thing in it and lets the
//! page load only its own track (the file copied beside the page, or the one address's origin); no script but the
//! page's own, no frame, no form, no connection. The player is the browser's: play, pause and seek. It never loops,
//! and it starts by itself only when the caller asked (`autoplay`), and then once: a reload of the page does not play
//! it again.

use std::path::Path;

/// The largest sound file the player takes (bytes): it is copied beside the page.
pub const AUDIO_MAX: u64 = 100 * 1024 * 1024;
/// The track's name in the page's folder, before its extension.
pub const TRACK_BASE: &str = "track";
/// The panel's header when the caller gave no title.
pub const DEFAULT_TITLE: &str = "Audio";
/// The sound files the player plays, by extension.
pub const AUDIO_EXTS: [&str; 10] = [
    "mp3", "wav", "ogg", "oga", "opus", "m4a", "aac", "flac", "webm", "weba",
];

/// The extension of a sound file the player plays, lower case; None for anything else. Both names of a resolved
/// file must say so (as typed and as the file system names it), so a link cannot change what is played.
pub fn audio_ext(typed: &str, real: &str) -> Option<&'static str> {
    let of = |p: &str| {
        Path::new(p)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
    };
    let (a, b) = (of(typed)?, of(real)?);
    if a != b {
        return None;
    }
    AUDIO_EXTS.iter().copied().find(|e| *e == a)
}

/// The track's file name in the page's folder.
pub fn track_name(ext: &str) -> String {
    format!("{TRACK_BASE}.{ext}")
}

/// The origin of an address (`scheme://host[:port]`), for the page's policy.
pub fn origin_of(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    match u.origin() {
        o @ url::Origin::Tuple(..) => Some(o.ascii_serialization()),
        url::Origin::Opaque(_) => None,
    }
}

/// The page's policy: its own inline code and style, and media from its own folder or from `origin` only.
pub fn csp(origin: Option<&str>) -> String {
    let media = origin.map_or_else(|| "'self'".to_string(), str::to_string);
    format!(
        "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; media-src {media}; \
         img-src 'none'; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'"
    )
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

/// The player page for `src` (the track's file name, or a public address whose origin is `origin`), named `title`.
pub fn page(src: &str, title: &str, origin: Option<&str>, autoplay: bool) -> String {
    let (src, title) = (esc(src), esc(title));
    let policy = csp(origin);
    let auto = if autoplay { "true" } else { "false" };
    format!(
        r#"<!doctype html><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="{policy}"><meta http-equiv="x-dns-prefetch-control" content="off">
<meta name="color-scheme" content="light dark"><title>{title}</title>
<style>
html,body{{margin:0;height:100%;font:13px "Segoe UI Variable Text","Segoe UI",sans-serif;color:CanvasText;background:Canvas}}
main{{box-sizing:border-box;min-height:100%;display:flex;flex-direction:column;justify-content:center;gap:12px;padding:20px}}
h1{{margin:0;font-size:14px;font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}}
audio{{width:100%}}
p{{margin:0;opacity:.7}}
</style>
<main><h1>{title}</h1><audio id="a" controls preload="metadata" src="{src}"></audio><p id="n"></p></main>
<script>
(function(){{
  var a=document.getElementById('a'),n=document.getElementById('n');
  a.loop=false;
  a.addEventListener('error',function(){{n.textContent='This sound could not be played.';}});
  // Plays by itself only when asked, and once: a reload of the page does not play it again.
  var once=false;try{{once=sessionStorage.getItem('played')==='1';sessionStorage.setItem('played','1');}}catch(e){{}}
  if({auto}&&!once){{var p=a.play();if(p&&p.catch)p.catch(function(){{}});}}
  document.addEventListener('keydown',function(e){{
    if(e.target===a)return;
    if(e.key===' '){{e.preventDefault();a.paused?a.play():a.pause();}}
    else if(e.key==='ArrowLeft'){{a.currentTime=Math.max(0,a.currentTime-5);}}
    else if(e.key==='ArrowRight'){{a.currentTime=a.currentTime+5;}}
  }});
}})();
</script>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sound_file_is_known_by_both_its_names() {
        assert_eq!(audio_ext("C:/a/b.MP3", "C:/a/b.mp3"), Some("mp3"));
        assert_eq!(audio_ext("x.opus", "x.opus"), Some("opus"));
        assert_eq!(
            audio_ext("x.mp3", "x.exe"),
            None,
            "a link that changes kind"
        );
        assert_eq!(audio_ext("x.txt", "x.txt"), None);
        assert_eq!(audio_ext("noext", "noext"), None);
        assert_eq!(track_name("ogg"), "track.ogg");
    }

    #[test]
    fn the_policy_comes_first_and_lets_in_only_the_track() {
        let p = page("track.mp3", "Speech", None, false);
        assert!(p.starts_with(
            "<!doctype html><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\""
        ));
        assert!(p.contains("media-src 'self';"));
        assert!(p.contains("connect-src 'none'"));
        assert!(
            p.contains("frame-src 'none'")
                && p.contains("form-action 'none'")
                && p.contains("base-uri 'none'")
        );
        assert!(
            p.contains("a.loop=false") && !p.contains(" loop"),
            "the player never loops"
        );
        assert!(p.contains("var p=a.play()"));
        assert!(p.contains("if(false&&!once)"), "no autoplay unless asked");
        let net = page(
            "https://cdn.example.com/a.mp3",
            "T",
            origin_of("https://cdn.example.com/a.mp3").as_deref(),
            true,
        );
        assert!(net.contains("media-src https://cdn.example.com;"), "{net}");
        assert!(net.contains("if(true&&!once)"));
    }

    #[test]
    fn the_title_and_the_source_cannot_break_out_of_the_page() {
        let p = page("a\"><script>x</script>", "<b onmouseover=x>", None, false);
        assert!(!p.contains("<script>x"));
        assert!(!p.contains("<b onmouseover"));
        assert!(p.contains("&lt;b onmouseover=x&gt;"));
    }

    #[test]
    fn an_origin_is_scheme_host_and_a_port_that_is_not_the_default() {
        assert_eq!(
            origin_of("https://Example.com:8443/a/b.mp3?x").as_deref(),
            Some("https://example.com:8443")
        );
        assert_eq!(
            origin_of("https://example.com/a").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(origin_of("data:audio/mp3,xx"), None);
    }
}
