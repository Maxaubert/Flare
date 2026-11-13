# Flare Selawik, bundled (macOS only)

The macOS UI face (rule 1232). Windows draws its UI in Segoe UI, which may not ship on a Mac; Selawik is
Microsoft's open stand-in for Segoe UI with its letter widths. Max chose it on 2026-11-13 so the Mac's UI looks like
the PC's.

- Source: **Selawik 1.01** (Microsoft, released 2015-12-08), `Selawik_Release.zip` from
  https://github.com/microsoft/Selawik/releases/tag/1.01: `selawk.ttf` (sha256
  `e9d98518d8ac2817782a9a382430463a2e0793ea68350b695bb727d9a830ee1c`), `selawksb.ttf`
  (`0a9e9d0549a10f24bef9b3a29e06fe6e0b5c21e7a784c503b048a307841a7783`) and `selawkb.ttf`
  (`f0db5e174a90e0956ad7d2844bdca1d5e6da92ec65b2c04e57ba9b180668c904`).
- Licence: SIL Open Font License 1.1 (`OFL.txt`, from the Selawik repository), (c) 2015 Microsoft Corporation, with
  Reserved Font Name Selawik.
- Modified: `tools/fonts/make-flare-selawik.py` sets each face's hhea ascent and descent to 2210 and -514 (Segoe UI
  5.32's own hhea, and Selawik's own usWinAscent and usWinDescent; Selawik's hhea is 2027 and -431, a 1.2 em line
  against Segoe UI's 1.33) and, because the OFL reserves the name Selawik for the original, renames the family
  "Flare Selawik". No glyph changes; the script keeps the original timestamps, so a rebuild gives the same bytes.
- Files: `flare-selawik-regular.ttf` (400), `flare-selawik-semibold.ttf` (600), `flare-selawik-bold.ttf` (700).
  Flare's UI asks for 400, 500, 600, 650 and 700; 500 draws as 400 and 650 as 700, as Segoe UI's own weights do.
- Compiled into the macOS build only (`crates/flare/src/ui_font.rs`, `include_bytes!`) and registered with Slint's
  font collection at start; a Windows build never reads these files.
- Glyphs: Selawik covers Latin-1, Latin Extended-A, the dashes, curly quotes, the bullet and U+2026 (Slint's elide).
  The Mac's key symbols (U+2318, U+2303, U+2325, U+21E7) and CJK come from the system's faces by per-character
  fallback, held by `ui_font::tests`.
