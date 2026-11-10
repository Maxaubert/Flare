# three.js, bundled

`viewer.js` is Flare's 3D model viewer (issue #36, `show_model`), built once from `tools/model-viewer/src/viewer.js` with
**three 0.170.0** (npm, published 2024-10-31, more than two weeks before it was added), MIT licence, (c) three.js authors (`LICENSE`
beside it) and **esbuild 0.24.0** (build only; none of it ships).

- Sources: `https://registry.npmjs.org/three/-/three-0.170.0.tgz` (npm integrity
  `sha512-FQK+LEpYc0fBD+J8g6oSEyyNzjp+Q7Ks1C568WWaoMRLW+TkNNWmenWeGgJjV105Gd+p/2ql1ZcjYvNiPZBhuQ==`), pinned exactly in
  `tools/model-viewer/package.json` and `package-lock.json`. Bundled from it: the core, `OrbitControls`, `GLTFLoader`, `STLLoader` and
  `RoomEnvironment`. No Draco, KTX2 or other WASM decoder is in the bundle.
- SHA-256 `f78d1b648e480cb0beb250741071c699945ae04d6494903ea78911868dd41227`, 590,286 bytes. A test
  (`crates/flare/src/ctl_app.rs`, `the_bundled_model_viewer_is_the_pinned_build`) holds the hash, so a changed file fails until this page
  and the notices say so. The same test refuses `eval`, `new Function`, `WebAssembly` and workers: the page's policy would refuse them too.
- Compiled into `flare.exe` with `include_bytes!` (`ctl_app::MODEL_VIEWER`). At each `view.model` it is copied into the model's own
  folder (`apps\a<n>\viewer.js`) and the page loads it from its own host (`script-src 'self'`). Never fetched at run time.
- To rebuild or update: take a three release at least two weeks old, set it in `package.json`, then in `tools/model-viewer`:
  `npm install --ignore-scripts`, `npm run build`. Replace `LICENSE` if it changed, the version and hash here, in `about.hbs` (the hand
  entry) and `THIRD-PARTY-NOTICES.md`, the line in `README.md` (Dependencies) and the hash in the test; then run the e2e `model`.
- `.gitattributes` marks it `-text`, so no checkout converts its line endings.
