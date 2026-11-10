// The 3D model viewer Flare writes beside the page of `show_model` (assets/three/README.md says how it is built).
// The page runs under `script-src 'self'; connect-src 'self'` (crates/flare-surfaces/src/model.rs), so nothing here may
// use eval, a worker, a blob: or data: fetch, or anything but the file beside the page. It fetches that file itself.
//
// `window.__flareModel` is a promise of `{ok: true, triangles, textures}` or `{ok: false, code, error}`, where `code` is one of
// the keys of `MESSAGES`; every failure also shows its message in the panel.

import {
  Box3,
  Color,
  DirectionalLight,
  GridHelper,
  Mesh,
  MeshStandardMaterial,
  PerspectiveCamera,
  PMREMGenerator,
  Scene,
  Sphere,
  TextureLoader,
  Vector3,
  WebGLRenderer,
} from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { RoomEnvironment } from 'three/addons/environments/RoomEnvironment.js';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { STLLoader } from 'three/addons/loaders/STLLoader.js';

/** The most triangles the viewer draws (the spec's limit, so a panel never freezes the machine). */
const TRIANGLES_MAX = 5_000_000;

const MESSAGES = {
  webgl: 'This panel cannot draw 3D: WebGL is not available.',
  big: 'This model has too many triangles to show (the limit is 5 million).',
  empty: 'This file has no geometry to show.',
  external: 'This model refers to separate files, which the viewer does not load. Export it as a single .glb file.',
  corrupt: 'This file could not be read as a 3D model.',
  context: 'The graphics context was lost. Close this panel and show the model again.',
};

class ModelError extends Error {
  constructor(code, detail) {
    super(detail || MESSAGES[code]);
    this.code = code;
  }
}

/** Only `data:` content can be read under the page's policy; anything else is another file. */
const isData = (uri) => typeof uri === 'string' && uri.startsWith('data:');

function decodeDataBuffer(uri) {
  const comma = uri.indexOf(',');
  if (comma < 0 || !uri.slice(0, comma).endsWith(';base64')) {
    throw new ModelError('corrupt', 'a buffer is not base64 data');
  }
  const text = atob(uri.slice(comma + 1));
  const bytes = new Uint8Array(text.length);
  for (let i = 0; i < text.length; i++) bytes[i] = text.charCodeAt(i);
  return bytes.buffer;
}

/**
 * Fits three's glTF parser to the page's policy. Its default image loader fetches a blob: address (connect-src refuses
 * that), an image element does not (img-src allows it); its buffer loader fetches a data: address the same way.
 */
function underThePolicy(parser) {
  const json = parser.json;
  for (const entry of [...(json.buffers || []), ...(json.images || [])]) {
    if (entry.uri !== undefined && !isData(entry.uri)) throw new ModelError('external');
  }
  parser.textureLoader = new TextureLoader(parser.options.manager);
  const loadBuffer = parser.loadBuffer.bind(parser);
  parser.loadBuffer = (index) => {
    const uri = json.buffers[index].uri;
    return isData(uri) ? Promise.resolve().then(() => decodeDataBuffer(uri)) : loadBuffer(index);
  };
  return { name: 'FLARE_under_the_policy' };
}

async function readGltf(bytes) {
  const loader = new GLTFLoader();
  loader.register(underThePolicy);
  return (await loader.parseAsync(bytes, '')).scene;
}

function readStl(bytes) {
  const geometry = new STLLoader().parse(bytes);
  // The loader always sets normals, copied from the file, and STL files commonly leave them at zero, which the shader cannot
  // light. The corners' order already decides which side is drawn, so the normals come from it. The geometry has no index,
  // so each facet stays flat.
  geometry.computeVertexNormals();
  const material = new MeshStandardMaterial({
    color: geometry.hasColors ? 0xffffff : 0xb4b8c0,
    metalness: 0.1,
    roughness: 0.65,
    vertexColors: Boolean(geometry.hasColors),
  });
  return new Mesh(geometry, material);
}

/** One row per format: a new format is one row here, one in `MODEL_EXTS`, and its loader. */
const FORMATS = { glb: readGltf, gltf: readGltf, stl: readStl };

function countTriangles(root) {
  let triangles = 0;
  let drawable = false;
  const maps = new Set();
  root.traverse((o) => {
    for (const m of [o.material].flat()) if (m && m.map) maps.add(m.map);
    const position = o.geometry && o.geometry.getAttribute('position');
    if (!position || position.count === 0) return;
    drawable = true;
    if (!o.isMesh) return;
    const corners = o.geometry.index ? o.geometry.index.count : position.count;
    triangles += Math.floor(corners / 3) * (o.isInstancedMesh ? o.count : 1);
  });
  return { triangles, drawable, textures: maps.size };
}

/** The file's bytes, turned into a checked scene graph. Needs no graphics, so a bad file is told apart from no WebGL. */
async function readModel(page) {
  const read = FORMATS[page.ext];
  if (!read) throw new ModelError('corrupt', 'an unknown format');
  let root;
  try {
    const answer = await fetch(page.file);
    if (!answer.ok) throw new Error(`the file answered ${answer.status}`);
    root = await read(await answer.arrayBuffer());
  } catch (e) {
    if (e instanceof ModelError) throw e;
    throw new ModelError('corrupt', String((e && e.message) || e));
  }
  const { triangles, drawable, textures } = countTriangles(root);
  if (triangles > TRIANGLES_MAX) throw new ModelError('big');
  if (!drawable) throw new ModelError('empty');
  root.updateMatrixWorld(true);
  const box = new Box3().setFromObject(root);
  const size = box.getSize(new Vector3());
  if (box.isEmpty() || ![size.x, size.y, size.z].every(Number.isFinite)) throw new ModelError('corrupt', 'its bounds are not finite');
  const sphere = box.getBoundingSphere(new Sphere());
  if (!(sphere.radius > 0)) throw new ModelError('empty');
  return { root, triangles, textures, box, sphere };
}

const COLOURS = {
  dark: { background: 0x1f2125, grid: 0x5a5f69, gridMinor: 0x34373d },
  light: { background: 0xf2f3f5, grid: 0x9aa0aa, gridMinor: 0xd4d7dc },
};

function show(model, page) {
  const { root, box, sphere } = model;
  let renderer;
  try {
    renderer = new WebGLRenderer({ canvas: page.canvas, antialias: true });
  } catch {
    throw new ModelError('webgl');
  }
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));

  const scene = new Scene();
  const environment = new PMREMGenerator(renderer).fromScene(new RoomEnvironment(), 0.04);
  scene.environment = environment.texture;
  const key = new DirectionalLight(0xffffff, 0.8);
  key.position.set(1, 2, 1.5);
  scene.add(key, root);

  const camera = new PerspectiveCamera(40, 1, sphere.radius / 1000, sphere.radius * 200);
  const controls = new OrbitControls(camera, page.canvas);
  controls.minDistance = sphere.radius / 100;
  controls.maxDistance = sphere.radius * 60;

  const render = () => renderer.render(scene, camera);

  let grid = null;
  let gridOn = true;
  const dark = window.matchMedia('(prefers-color-scheme: dark)');
  function paintTheme() {
    const colours = dark.matches ? COLOURS.dark : COLOURS.light;
    scene.background = new Color(colours.background);
    if (grid) {
      scene.remove(grid);
      grid.geometry.dispose();
      grid.material.dispose();
    }
    grid = new GridHelper(sphere.radius * 4, 20, colours.grid, colours.gridMinor);
    grid.position.set(sphere.center.x, box.min.y, sphere.center.z);
    grid.visible = gridOn;
    scene.add(grid);
  }

  function frameTheModel() {
    const vertical = (camera.fov * Math.PI) / 360;
    const horizontal = Math.atan(Math.tan(vertical) * camera.aspect);
    const distance = (sphere.radius / Math.sin(Math.min(vertical, horizontal))) * 1.1;
    camera.position.copy(sphere.center).add(new Vector3(0.6, 0.45, 1).normalize().multiplyScalar(distance));
    controls.target.copy(sphere.center);
    controls.update();
  }

  function fitTheCanvas() {
    const { clientWidth: w, clientHeight: h } = page.canvas;
    if (w === 0 || h === 0) return false;
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
    return true;
  }

  let framed = false;
  function resized() {
    if (!fitTheCanvas()) return;
    if (!framed) {
      framed = true;
      frameTheModel();
    }
    render();
  }

  paintTheme();
  // Drawn only when something changed, so a model left open costs no GPU time.
  controls.addEventListener('change', render);
  new ResizeObserver(resized).observe(page.canvas);
  resized();
  dark.addEventListener('change', () => {
    paintTheme();
    render();
  });
  page.canvas.addEventListener('dblclick', () => {
    frameTheModel();
    render();
  });
  page.canvas.addEventListener('webglcontextlost', (e) => {
    e.preventDefault();
    page.say(MESSAGES.context);
  });

  let wireframe = false;
  page.toggle('wire', false, () => {
    wireframe = !wireframe;
    root.traverse((o) => {
      if (o.isMesh) for (const m of [o.material].flat()) m.wireframe = wireframe;
    });
    render();
    return wireframe;
  });
  page.toggle('grid', true, () => {
    gridOn = !gridOn;
    grid.visible = gridOn;
    render();
    return gridOn;
  });

  // For the e2e: draws now and gives the brightness (0 to 255) of every pixel that is not the background.
  function drawnBrightness() {
    resized();
    render();
    const gl = renderer.getContext();
    const { width, height } = gl.canvas;
    const pixels = new Uint8Array(width * height * 4);
    gl.readPixels(0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
    const [r, g, b] = pixels;
    const drawn = [];
    for (let i = 0; i < pixels.length; i += 4) {
      if (pixels[i] !== r || pixels[i + 1] !== g || pixels[i + 2] !== b) {
        drawn.push((pixels[i] + pixels[i + 1] + pixels[i + 2]) / 3);
      }
    }
    return drawn;
  }

  // How many pixels are not the background.
  window.__flareModelPixels = () => drawnBrightness().length;

  // How the model is lit: its mean brightness, and how far apart its lightest and darkest tenths are.
  window.__flareModelShade = () => {
    const drawn = drawnBrightness().sort((a, b) => a - b);
    if (drawn.length === 0) return { mean: 0, spread: 0 };
    const mean = drawn.reduce((sum, v) => sum + v, 0) / drawn.length;
    const tenth = Math.floor(drawn.length / 10);
    return { mean: Math.round(mean), spread: Math.round(drawn[drawn.length - 1 - tenth] - drawn[tenth]) };
  };
}

function thePage() {
  const message = document.getElementById('msg');
  const say = (text) => {
    message.textContent = text;
    message.hidden = false;
  };
  const toggle = (id, on, change) => {
    const button = document.getElementById(id);
    button.setAttribute('aria-pressed', String(on));
    button.hidden = false;
    button.addEventListener('click', () => button.setAttribute('aria-pressed', String(change())));
  };
  const canvas = document.getElementById('view');
  const { file, ext } = document.body.dataset;
  return { canvas, file, ext, say, toggle, clear: () => (message.hidden = true) };
}

window.__flareModel = (async () => {
  const page = thePage();
  try {
    const model = await readModel(page);
    try {
      show(model, page);
    } catch (e) {
      throw e instanceof ModelError ? e : new ModelError('webgl', String((e && e.message) || e));
    }
    page.clear();
    document.getElementById('hint').hidden = false;
    return { ok: true, triangles: model.triangles, textures: model.textures };
  } catch (e) {
    const code = e instanceof ModelError ? e.code : 'corrupt';
    page.say(MESSAGES[code]);
    return { ok: false, code, error: String((e && e.message) || e) };
  }
})();
