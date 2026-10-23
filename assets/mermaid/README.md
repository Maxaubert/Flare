# Mermaid, bundled

`mermaid.min.js` is the official `dist/mermaid.min.js` of the npm package **mermaid 11.17.2** (published 2026-08-25, more than
two weeks before it was added), MIT licence, (c) Knut Sveidqvist (`LICENSE` beside it). It is the build that draws the diagrams
an agent asks for with `show_diagram` (Surfaces stage 5, `docs/surfaces/`).

- Fetched from `https://registry.npmjs.org/mermaid/-/mermaid-11.17.2.tgz` (npm integrity
  `sha512-V6K3C8EBdEsPFZXSKMJe6ppQOENxuHARr9GvHX4hh47lAbhMRD9qf4oEK7LoaRQxULMa80/qt5gHO73aCleBBg==`), file `package/dist/mermaid.min.js`,
  kept byte for byte. SHA-256 `581ed7d74bd9048d0e3a91363927d72ef22942d7722546b27f7cc29e35390eb8`, 3,572,661 bytes. A test
  (`crates/flare/src/ctl_app.rs`, `the_bundled_mermaid_is_the_pinned_build`) holds the hash, so a changed file fails until this page
  and the notices say so.
- Compiled into `flare.exe` with `include_bytes!` (`ctl_app::MERMAID`), not shipped beside it: nothing to lose in the installer or
  the portable zip, nothing to find at run time, and the e2e runs the same bytes. It costs 3.5 MB of the exe, mapped from disk and
  touched only when a diagram is drawn. At each `view.diagram` it is copied into the diagram's own folder (`apps\a<n>\mermaid.min.js`)
  and the page loads it from its own host (`script-src 'self'`).
- Never fetched at run time and never from a CDN. The page runs with `securityLevel: 'strict'` and no network.
- To update: take a release at least two weeks old, replace the file, `LICENSE` if it changed, the version and hash here, in
  `about.hbs` (the hand entry), regenerate `THIRD-PARTY-NOTICES.md` (`cargo about generate about.hbs`), the line in `README.md`
  (Dependencies) and the hash in the test; then run the e2e `diagram` and `mcpDiagram`.
- `.gitattributes` marks it `-text`, so no checkout converts its line endings.
