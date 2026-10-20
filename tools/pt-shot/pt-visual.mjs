// The visual tour's Prism Terminal half (audit 2026-10-20): the same surfaces as Flare's `visualTour`
// scenario (tools/e2e/src/scenarios/visual.rs), written to <out>/<surface>/<pct>-pt/.
//
//   node pt-visual.mjs --out <folder> [--scales 1,1.5,2.25] [--surfaces startup,resume,sidebar,general]
//                      [--runs 5] [--src <installed PT folder>]
//
// SAFETY. It never launches the installed Prism Terminal: the installed folder is COPIED to a temp folder and
// the copy is launched with its own --user-data-dir and --e2e (window parked offscreen, unfocusable), through
// Playwright's Electron driver from the Prism repo's node_modules. Nothing under %APPDATA% is read or written;
// every process whose command line names the copy or the profile is reaped at the end, and nothing else is.
//
// WHAT IT CAPTURES (what Flare's tour does, with the page's own tools):
//  - startup / resume: the window's pixels as fast as page.screenshot can take them from the launch call on,
//    each changed picture kept with its time; plus first-contentful-paint and the ready times counted from the
//    PROCESS start (main's Date.now() - process.uptime()), so they sit beside Flare's `startup.trace`.
//    Playwright's own attach time is in the frame times (the pictures are timed from the launch call), the
//    process-start numbers are not.
//  - sidebar: Ctrl+Shift+B with a requestAnimationFrame recorder of the slot's width and the panel's left (the
//    series Flare keeps per frame), three collapses and three expands; and pictures at fixed offsets taken by
//    PAUSING every CSS transition at that time (document.getAnimations()), which is exact where Flare's
//    snapshots are within a few ms.
//  - general: tab strip, tab menu, help panel, Settings pages, themes (by localStorage and a reload, not the
//    wall), see-through (the slider).
// Not captured: the dictation pill and widget (they need the speech engine; Flare's side is `dictation`).
import { execFileSync } from 'node:child_process'
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { createHash } from 'node:crypto'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'

const HERE = dirname(fileURLToPath(import.meta.url))
const PRISM_MODULES = 'C:\\Users\\Admin\\Documents\\Claude\\Github\\Prism\\node_modules'
const require = createRequire(join(PRISM_MODULES, 'noop.js'))
const { _electron: electron } = require('playwright-core')

const INSTALLED = 'C:\\Users\\Admin\\AppData\\Local\\Programs\\PrismTerminal'
const W = 1000
const H = 640
const OFFSETS = [0, 20, 40, 60, 80, 100, 130, 160, 200, 260]
const EASE = [0.16, 1, 0.3, 1]
const SLIDE_MS = 240

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

function parseArgs(argv) {
  const o = { scales: '1,1.5,2.25', surfaces: 'startup,resume,sidebar,general', runs: 5, src: INSTALLED }
  for (let i = 0; i < argv.length; i++) if (argv[i].startsWith('--')) o[argv[i].slice(2)] = argv[++i]
  o.runs = Number(o.runs)
  return o
}

/** Kill only processes whose command line names `needle` (our copy or our profile). */
function reap(needle) {
  try {
    const q =
      "Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '*" +
      needle.replace(/'/g, "''") +
      "*' } | ForEach-Object { $_.ProcessId }"
    const out = execFileSync('powershell.exe', ['-NoProfile', '-Command', q], { encoding: 'utf8', windowsHide: true })
    for (const pid of out.split(/\s+/).filter(Boolean)) {
      try {
        execFileSync('taskkill', ['/PID', pid, '/T', '/F'], { stdio: 'ignore' })
      } catch {}
    }
  } catch {}
}

function park({ BrowserWindow }) {
  for (const w of BrowserWindow.getAllWindows()) {
    w.setOpacity(0)
    w.setPosition(-4000, -4000)
  }
}

/** The CSS cubic-bezier progress at time fraction t. */
function bezier(t) {
  t = Math.min(1, Math.max(0, t))
  const at = (a, b, s) => 3 * a * (1 - s) ** 2 * s + 3 * b * (1 - s) * s * s + s ** 3
  let lo = 0
  let hi = 1
  for (let i = 0; i < 60; i++) {
    const mid = (lo + hi) / 2
    if (at(EASE[0], EASE[2], mid) < t) lo = mid
    else hi = mid
  }
  return at(EASE[1], EASE[3], (lo + hi) / 2)
}

/** The same definitions as tools/e2e/src/slide.rs `analyse`: rows are [age, value] (css px). */
function analyse(rows, pressAge) {
  if (rows.length < 2) return null
  const from = rows[0][1]
  const to = rows[rows.length - 1][1]
  if (Math.abs(to - from) < 0.5) return null
  const i0 = rows.findIndex((r) => Math.abs(r[1] - from) > 0.25)
  if (i0 < 0) return null
  let iEnd = i0
  for (let i = rows.length - 1; i >= 0; i--) {
    if (Math.abs(rows[i][1] - to) > 0.5) {
      iEnd = Math.min(i + 1, rows.length - 1)
      break
    }
  }
  iEnd = Math.max(iEnd, i0)
  const t0 = rows[i0][0]
  const prog = (v) => (v - from) / (to - from)
  const curve = []
  for (let i = Math.max(0, i0 - 1); i <= iEnd; i++) curve.push([rows[i][0] - t0, prog(rows[i][1])])
  const gaps = []
  for (let i = i0; i <= iEnd; i++) if (i > 0) gaps.push(rows[i][0] - rows[i - 1][0])
  const cross = (p) => {
    for (let i = 1; i < curve.length; i++) {
      const [ta, pa] = curve[i - 1]
      const [tb, pb] = curve[i]
      if (pa < p && pb >= p) return ta + ((p - pa) / (Math.abs(pb - pa) < 1e-9 ? 1 : pb - pa)) * (tb - ta)
    }
    return curve[curve.length - 1][0]
  }
  const dev = (off) => Math.max(...curve.map(([t, p]) => Math.abs(p - bezier((t - off) / SLIDE_MS))))
  let best = [Infinity, 0]
  for (let off = -30; off <= 30; off++) {
    const d = dev(off)
    if (d < best[0]) best = [d, off]
  }
  const r = (v) => Math.round(v * 100) / 100
  return {
    frames: iEnd + 1 - Math.max(0, i0 - 1),
    from,
    to,
    input_ms: r(t0 - pressAge),
    t50_ms: r(cross(0.5)),
    t90_ms: r(cross(0.9)),
    t99_ms: r(cross(0.99)),
    settle_ms: r(rows[iEnd][0] - t0),
    longest_gap_ms: r(Math.max(0, ...gaps)),
    over_20: gaps.filter((g) => g > 20).length,
    over_33: gaps.filter((g) => g > 33).length,
    max_render_ms: null,
    dev_at_first: Math.round(dev(0) * 1000) / 1000,
    dev_best: Math.round(best[0] * 1000) / 1000,
    best_offset_ms: best[1],
    curve: curve.map(([t, p]) => [r(t), Math.round(p * 1000) / 1000]),
  }
}

class Lab {
  constructor(o) {
    this.o = o
    this.root = mkdtempSync(join(tmpdir(), 'pt-visual-'))
    this.copy = join(this.root, 'app')
    cpSync(o.src, this.copy, { recursive: true })
    this.exe = join(this.copy, 'PrismTerminal.exe')
    if (!existsSync(this.exe)) throw new Error(`no PrismTerminal.exe in the copy of ${o.src}`)
    this.n = 0
  }

  /** A fresh world: a profile and three folders. */
  world() {
    const w = join(this.root, `w${++this.n}`)
    const profile = join(w, 'profile')
    mkdirSync(profile, { recursive: true })
    const dirs = ['alpha', 'beta', 'gamma'].map((d) => {
      const p = join(w, d)
      mkdirSync(p)
      return p
    })
    return { w, profile, dirs }
  }

  async launch(world, scale, args = []) {
    const began = Date.now()
    const app = await electron.launch({
      executablePath: this.exe,
      args: [`--user-data-dir=${world.profile}`, '--e2e', `--force-device-scale-factor=${scale}`, ...args],
      env: { ...process.env },
    })
    const page = await app.firstWindow()
    await app.evaluate(park)
    return { app, page, began }
  }

  async close(h) {
    await h.app.close().catch(() => {})
  }

  cleanup() {
    reap(this.root)
    rmSync(this.root, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 })
  }
}

async function size(h) {
  for (let k = 0, ask = [W, H]; k < 4; k++) {
    await h.app.evaluate(({ BrowserWindow }, [w, hh]) => {
      for (const win of BrowserWindow.getAllWindows()) win.setContentSize(w, hh)
    }, ask)
    await sleep(250)
    const [iw, ih] = await h.page.evaluate(() => [innerWidth, innerHeight])
    if (iw === W && ih === H) break
    ask = [ask[0] + (W - iw), ask[1] + (H - ih)]
  }
  await h.app.evaluate(park)
}

const PROMPT = /[A-Za-z]:\\[^>\n]*>\s*$|PS [^>]*>\s*$/
const promptShowing = (page) =>
  page.evaluate((src) => new RegExp(src).test((document.querySelector('.xterm .xterm-rows')?.textContent ?? '').trimEnd()), PROMPT.source).catch(() => false)

async function tabCount(page) {
  return page.evaluate(() => document.querySelectorAll('[data-tab]').length).catch(() => 0)
}

async function readyApp(h, tabs) {
  const end = Date.now() + 60000
  // A fresh profile opens on the welcome screen: start the first terminal.
  while (Date.now() < end) {
    if ((await tabCount(h.page)) >= tabs && (await promptShowing(h.page))) {
      await sleep(300)
      return true
    }
    const welcome = h.page.getByText('New terminal', { exact: true }).first()
    if (!(await h.page.locator('.xterm').count().catch(() => 0)) && (await welcome.count().catch(() => 0)) && tabs === 1)
      await welcome.click({ force: true }).catch(() => {})
    await sleep(40)
  }
  return false
}

const save = (dir, v) => {
  mkdirSync(dir, { recursive: true })
  writeFileSync(join(dir, 'numbers.json'), JSON.stringify(v, null, 2))
}

const shot = (h, file) => h.page.screenshot({ path: file, clip: { x: 0, y: 0, width: W, height: H } }).then(() => true).catch(() => false)

/** Writes the layout and the session a resume needs into a closed profile. */
async function prepResume(lab, world, scale) {
  const h = await lab.launch(world, scale, [world.dirs[0]])
  await readyApp(h, 1)
  await h.page.evaluate(() => {
    localStorage.setItem('prism.window.tabLayout', 'vertical')
    localStorage.setItem('prism.window.sidebar', 'open')
  })
  await lab.close(h)
  writeFileSync(join(world.profile, 'tabs.json'), JSON.stringify({ tabs: world.dirs.map((cwd) => ({ cwd })), active: 1 }))
}

/** The times from process start: first contentful paint, and the helper's own. */
async function procClock(h) {
  const startEpoch = await h.app.evaluate(() => Date.now() - process.uptime() * 1000)
  const fcp = await h.page
    .evaluate(() => {
      const p = performance.getEntriesByType('paint').find((e) => e.name === 'first-contentful-paint')
      return p ? performance.timeOrigin + p.startTime : null
    })
    .catch(() => null)
  return { startEpoch, fcp_since_process_ms: fcp === null ? null : fcp - startEpoch }
}

/** One watched launch: every changed picture from the launch call on, until ready and still for 700 ms. */
async function watched(lab, surface, scale, dir) {
  const world = lab.world()
  const tabsWanted = surface === 'resume' ? 3 : 1
  let args = [world.dirs[0]]
  if (surface === 'resume') {
    await prepResume(lab, world, scale)
    args = []
  }
  const frames = []
  mkdirSync(join(dir, 'frames'), { recursive: true })
  const h = await lab.launch(world, scale, args)
  const tWindow = Date.now() - h.began
  const clock0 = await procClock(h)
  let last = null
  let changed = Date.now()
  let readyAt = null
  const timeline = []
  const end = Date.now() + 40000
  while (Date.now() < end) {
    const t = Date.now()
    const buf = await h.page.screenshot({ clip: { x: 0, y: 0, width: W, height: H } }).catch(() => null)
    if (buf) {
      const hash = createHash('md5').update(buf).digest('hex')
      if (hash !== last) {
        last = hash
        changed = Date.now()
        const n = frames.length
        const name = `${String(n).padStart(3, '0')}-t${String(t - h.began).padStart(5, '0')}ms.png`
        if (n < 80) writeFileSync(join(dir, 'frames', name), buf)
        frames.push({ n, t_ms: t - h.began, file: name })
      }
    }
    const st = await h.page
      .evaluate(() => ({
        tabs: document.querySelectorAll('[data-tab]').length,
        front: [...document.querySelectorAll('[data-tab]')].findIndex((e) => e.querySelector('[role="tab"][aria-selected="true"]')),
        titles: [...document.querySelectorAll('[data-tab]')].map((e) => (e.textContent ?? '').trim()),
      }))
      .catch(() => null)
    if (st) {
      const e = { t_ms: Date.now() - h.began, tabs: st.tabs, front: st.front, cwds: st.titles }
      const l = timeline[timeline.length - 1]
      if (!l || l.tabs !== e.tabs || l.front !== e.front || JSON.stringify(l.cwds) !== JSON.stringify(e.cwds)) timeline.push(e)
      if (st.tabs >= tabsWanted && !readyAt && (await promptShowing(h.page))) readyAt = Date.now() - h.began
    }
    // The cursor blinks forever, so "still" is never true: stop a moment after the prompt shows.
    if (readyAt && Date.now() - h.began - readyAt > 1200) break
  }
  const clock = await procClock(h)
  const front = await h.page.evaluate(() => [...document.querySelectorAll('[data-tab]')].findIndex((e) => e.querySelector('[role="tab"][aria-selected="true"]'))).catch(() => -1)
  await lab.close(h)
  return { frames, timeline, readyAt, tWindow, clock, front, startEpoch: clock0.startEpoch }
}

/** Launches without a picture observer; the times from process start. */
async function timing(lab, surface, scale, runs) {
  const out = []
  for (let i = 0; i <= runs; i++) {
    const world = lab.world()
    let args = [world.dirs[0]]
    if (surface === 'resume') {
      await prepResume(lab, world, scale)
      args = []
    }
    const h = await lab.launch(world, scale, args)
    const wanted = surface === 'resume' ? 3 : 1
    const startEpoch = await h.app.evaluate(() => Date.now() - process.uptime() * 1000)
    let tabsAt = null
    let promptAt = null
    const end = Date.now() + 40000
    while (Date.now() < end && !promptAt) {
      if ((await tabCount(h.page)) >= wanted) {
        tabsAt ??= Date.now() - startEpoch
        if (await promptShowing(h.page)) promptAt = Date.now() - startEpoch
      }
      await sleep(10)
    }
    const fcp = await procClock(h)
    await lab.close(h)
    if (i === 0) continue // the first launch warms the disk cache and is thrown away
    out.push({ launch_to_window_ms: h.began ? null : null, fcp_since_process_ms: fcp.fcp_since_process_ms, tabs_since_process_ms: tabsAt, prompt_since_process_ms: promptAt })
  }
  const med = (k) => {
    const v = out.map((r) => r[k]).filter((x) => typeof x === 'number').sort((a, b) => a - b)
    return v.length ? (v.length % 2 ? v[(v.length - 1) / 2] : (v[v.length / 2 - 1] + v[v.length / 2]) / 2) : null
  }
  return { runs: out, median: { fcp_since_process_ms: med('fcp_since_process_ms'), tabs_since_process_ms: med('tabs_since_process_ms'), prompt_since_process_ms: med('prompt_since_process_ms') } }
}

// ---- the sidebar ----------------------------------------------------------------------------------------

const REC = () => {
  window.__rec = { rows: [], on: true }
  const slot = document.querySelector('[data-sidebar-slot]')
  const nav = document.querySelector('[data-tab-sidebar]')
  const t0 = performance.now()
  const tick = (t) => {
    if (!window.__rec.on) return
    window.__rec.rows.push([t - 0, slot.getBoundingClientRect().width, nav.getBoundingClientRect().left])
    requestAnimationFrame(tick)
  }
  requestAnimationFrame(tick)
  return t0
}

async function sidebarCell(lab, scale, dir) {
  const world = lab.world()
  await prepResume(lab, world, scale)
  const h = await lab.launch(world, scale, [])
  await size(h)
  const ok = await readyApp(h, 3)
  await h.page.locator('.xterm').first().click({ force: true }).catch(() => {})
  await sleep(600)
  const state = () => h.page.evaluate(() => document.querySelector('[data-tab-sidebar]')?.getAttribute('data-sidebar-mode'))
  const cols = () => h.page.evaluate(() => document.querySelector('.xterm .xterm-rows')?.children.length ?? 0)
  const run = async (wantOpen) => {
    const t0 = await h.page.evaluate(REC)
    const press = await h.page.evaluate(() => performance.now())
    await h.page.keyboard.press('Control+Shift+B')
    await sleep(650)
    const rows = await h.page.evaluate(() => {
      window.__rec.on = false
      return window.__rec.rows
    })
    const slot = rows.map((r) => [r[0], r[1]])
    const left = rows.map((r) => [r[0], r[2]])
    return { slot: analyse(slot, press), panel: analyse(left, press), frames_total: rows.length, rows: rows.slice(0, 120), mode: await state(), wantOpen }
  }
  const collapse = []
  const expand = []
  for (let i = 0; i < 3; i++) {
    collapse.push(await run(false))
    expand.push(await run(true))
  }
  // Pictures: pause every transition at the offset.
  const snaps = []
  mkdirSync(join(dir, 'frames'), { recursive: true })
  for (const [name, toOpen] of [['collapse', false], ['expand', true]]) {
    if (toOpen) {
      await h.page.keyboard.press('Control+Shift+B')
      await sleep(700)
    }
    for (let k = 0; k < OFFSETS.length; k++) {
      await h.page.keyboard.press('Control+Shift+B')
      const seen = await h.page.evaluate((off) => {
        const a = document.getAnimations().filter((x) => x instanceof CSSTransition)
        a.forEach((x) => {
          x.pause()
          x.currentTime = off
        })
        window.__held = a
        return {
          anims: a.length,
          slot: document.querySelector('[data-sidebar-slot]').getBoundingClientRect().width,
          left: document.querySelector('[data-tab-sidebar]').getBoundingClientRect().left,
        }
      }, OFFSETS[k])
      await sleep(60)
      const file = `${name}-${String(k).padStart(2, '0')}-t${String(OFFSETS[k]).padStart(3, '0')}ms-slot${String(Math.round(seen.slot)).padStart(3, '0')}.png`
      const taken = await shot(h, join(dir, 'frames', file))
      snaps.push({ name, k, target_ms: OFFSETS[k], slot: seen.slot, panel_left: seen.left, anims: seen.anims, taken, file })
      await h.page.evaluate(() => window.__held.forEach((x) => x.finish()))
      await sleep(700)
      await h.page.keyboard.press('Control+Shift+B')
      await sleep(700)
    }
  }
  const med = (list, k) => {
    const v = list.map((r) => r.slot?.[k]).filter((x) => typeof x === 'number').sort((a, b) => a - b)
    return v.length ? v[Math.floor(v.length / 2)] : null
  }
  save(dir, {
    surface: 'sidebar',
    renderer: 'pt',
    ready: ok,
    window_css: [W, H],
    cols: { rows_in_dom: await cols() },
    collapse,
    expand,
    medians: {
      collapse_slot: { longest_gap_ms: med(collapse, 'longest_gap_ms'), frames: med(collapse, 'frames'), t99_ms: med(collapse, 't99_ms'), settle_ms: med(collapse, 'settle_ms'), dev_best: med(collapse, 'dev_best') },
      expand_slot: { longest_gap_ms: med(expand, 'longest_gap_ms'), frames: med(expand, 'frames'), t99_ms: med(expand, 't99_ms'), settle_ms: med(expand, 'settle_ms'), dev_best: med(expand, 'dev_best') },
    },
    snapshots: snaps,
  })
  await lab.close(h)
}

// ---- the general pass -----------------------------------------------------------------------------------

async function generalCell(lab, scale, dir) {
  const world = lab.world()
  const h = await lab.launch(world, scale, world.dirs)
  await size(h)
  const ok = await readyApp(h, 3)
  const notes = []
  let n = 0
  mkdirSync(join(dir, 'frames'), { recursive: true })
  const take = async (name) => {
    const f = `${String(++n).padStart(2, '0')}-${name}.png`
    const t = await shot(h, join(dir, 'frames', f))
    notes.push({ name, taken: t })
  }
  if (ok) {
    await take('tabs-strip')
    await h.page.locator('[data-tab]').first().click({ button: 'right', force: true }).catch(() => {})
    await sleep(300)
    await take('tab-menu')
    await h.page.keyboard.press('Escape')
    await sleep(250)
    await h.page.locator('.xterm').first().click({ force: true }).catch(() => {})
    await h.page.keyboard.press('F1')
    await h.page.locator('[data-help-panel][data-help-open]').waitFor({ timeout: 6000 }).catch(() => {})
    await sleep(900)
    await take('help-panel')
    await h.page.keyboard.press('Escape')
    await sleep(500)
    await h.page.locator('[data-title-settings]').click().catch(() => {})
    await h.page.locator('[data-settings-page]').waitFor({ timeout: 10000 }).catch(() => {})
    await sleep(400)
    await take('settings-first-page')
    for (const id of ['appearance', 'dictation', 'about']) {
      await h.page.locator(`[data-settings-tab="${id}"]`).click().catch(() => {})
      await sleep(500)
      await take(`settings-${id}`)
    }
    await h.page.locator('[data-title-settings]').click().catch(() => {})
    await sleep(400)
    for (const theme of ['nord', 'paper', 'pt-default']) {
      await h.page.evaluate((t) => localStorage.setItem('prism.term.theme', t), theme)
      await h.page.reload()
      await h.page.waitForFunction(() => !!window.prism, null, { timeout: 60000 }).catch(() => {})
      await h.app.evaluate(park)
      await take(`theme-${theme}-at-once`)
      await readyApp(h, 3)
      await sleep(450)
      await take(`theme-${theme}-settled`)
    }
    // see-through
    await h.page.locator('[data-title-settings]').click().catch(() => {})
    await h.page.locator('[data-settings-tab="appearance"]').click().catch(() => {})
    const slider = h.page.locator('[data-pref="term-acrylic"] input[type="range"]').first()
    await slider.fill('40').catch(() => {})
    await sleep(400)
    await h.page.locator('[data-title-settings]').click().catch(() => {})
    await sleep(500)
    await take('see-through-40')
  }
  save(dir, { surface: 'general', renderer: 'pt', ready: ok, pictures: notes })
  await lab.close(h)
}

async function main() {
  const o = parseArgs(process.argv.slice(2))
  if (!o.out) throw new Error('--out <folder> is required')
  const out = resolve(o.out)
  const want = new Set(o.surfaces.split(','))
  const lab = new Lab(o)
  const version = execFileSync('powershell.exe', ['-NoProfile', '-Command', `(Get-Item -LiteralPath '${lab.exe}').VersionInfo.ProductVersion`], { encoding: 'utf8' }).trim()
  mkdirSync(out, { recursive: true })
  writeFileSync(join(out, 'PT-VERSION.txt'), version + '\n')
  console.log(`Prism Terminal ${version} (copy at ${lab.copy})`)
  const timings = {}
  try {
    for (const s of o.scales.split(',').map(Number)) {
      const pct = Math.round(s * 100)
      for (const surface of ['startup', 'resume']) {
        if (!want.has(surface)) continue
        const dir = join(out, surface, `${pct}-pt`)
        rmSync(dir, { recursive: true, force: true })
        console.log(`${surface} ${pct}%`)
        const t = await timing(lab, surface, s, o.runs)
        timings[`${surface}-${pct}-pt`] = t
        const w = await watched(lab, surface, s, dir)
        save(dir, { surface, pct, renderer: 'pt', version, ...w, timing: t })
      }
      if (want.has('sidebar')) {
        const dir = join(out, 'sidebar', `${pct}-pt`)
        rmSync(join(dir, 'frames'), { recursive: true, force: true })
        console.log(`sidebar ${pct}%`)
        await sidebarCell(lab, s, dir)
      }
      if (want.has('general')) {
        const dir = join(out, 'general', `${pct}-pt`)
        rmSync(join(dir, 'frames'), { recursive: true, force: true })
        console.log(`general ${pct}%`)
        await generalCell(lab, s, dir)
      }
    }
    writeFileSync(join(out, `timings-pt-${process.pid}.json`), JSON.stringify(timings, null, 2))
  } finally {
    lab.cleanup()
  }
  console.log('done')
}

main().catch((e) => {
  console.error('FAIL ' + (e?.stack ?? e))
  process.exit(1)
})
