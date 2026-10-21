// Reference capture of Prism Terminal's Settings page, in the same fixed state as Flare's
// `--scene settings`, so Flare's settings matrix (Task 51) can sit beside it. Throwaway profile,
// window parked offscreen and unfocusable, the machine's own profile folders never touched.
// The launch block is the one of pt-shot.mjs.
//
//   node pt-settings.mjs --out <folder> [--scales 1,1.5,2.25] [--only <text>] [--size 1000x640]
//                        [--exe <PrismTerminal.exe>]
//   node pt-settings.mjs --list        (the job list as JSON; the matrix script reads it)
//   node pt-settings.mjs --selftest    (one Appearance image at scale 1; prints ok)
//
// The images are `<out>/<page>-<variant>-<pct>.png`. The fixed state is the one Flare's scene
// has: theme pt-default, font cascadia, size 100, edges hairline, tab layout horizontal, new tab
// mode folder, everything else default.
import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'

const HERE = dirname(fileURLToPath(import.meta.url))
const PRISM_MODULES = 'C:\\Users\\Admin\\Documents\\Claude\\Github\\Prism\\node_modules'
const DEFAULT_EXE = 'C:\\Users\\Admin\\AppData\\Local\\Programs\\PrismTerminal\\PrismTerminal.exe'
const PROFILE_PREFIX = 'pt-settings-profile-'
const REAL_PROFILES = ['PrismTerminalStable', 'PrismTerminal'].map((n) => join(process.env.APPDATA ?? '', n))

export const PAGES = ['appearance', 'terminal', 'agents', 'dictation', 'diagnostics', 'about']
export const SCALES3 = [1, 1.5, 2.25]
export const SCALES8 = [1, 1.25, 1.5, 1.75, 2, 2.25, 2.5, 3]
/** The accent that makes the theme dirty for the switch question (Flare's DIRTY_ACCENT). */
const DIRTY_ACCENT = '#e07a2f'
/** How far the pane is scrolled when the wall is shown open (Flare's WALL_SCROLL). */
const WALL_SCROLL = 330
/** The fixed store: what Flare's scene prepares, in this app's keys. */
const FIXED = {
  'prism.term.theme': 'pt-default',
  'prism.term.font': 'cascadia',
  'prism.term.fontPct': '100',
  'prism.window.edges': 'hairline',
  'prism.window.tabLayout': 'horizontal',
  'prism.newtab.mode': 'folder'
}

/**
 * Every image of the matrix, once. The Appearance page has no select (its rows are a pill, a
 * slider, colour fields and the wall), so "select" is only listed for Terminal.
 */
export function jobs() {
  const out = new Map()
  const add = (page, variant, s, extra = {}) => {
    const pct = Math.round(s * 100)
    const name = `${page}-${variant}-${pct}`
    if (!out.has(name)) out.set(name, { name, page, open: variant === 'plain' || variant === 'find' ? 'none' : variant, find: variant === 'find' ? 'colour' : '', scale: s, pct, ...extra })
  }
  for (const s of SCALES3) {
    for (const p of PAGES) add(p, 'plain', s)
    for (const v of ['picker', 'editor', 'ask', 'help', 'wall-expanded']) add('appearance', v, s)
    for (const v of ['select', 'help']) add('terminal', v, s)
    add('appearance', 'find', s)
  }
  for (const s of SCALES8) {
    add('appearance', 'plain', s)
    add('terminal', 'plain', s)
  }
  return [...out.values()]
}

function parseArgs(argv) {
  const o = { scales: '', only: '', size: '1000x640', selftest: false, list: false }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--selftest') o.selftest = true
    else if (a === '--list') o.list = true
    else if (a.startsWith('--')) o[a.slice(2)] = argv[++i]
  }
  if (o.selftest) {
    o.scales = '1'
    o.only = 'appearance-plain-100'
    o.out ??= mkdtempSync(join(tmpdir(), 'pt-settings-self-'))
  }
  return o
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

function stamp(p) {
  try {
    return statSync(p).mtimeMs
  } catch {
    return null
  }
}

function productVersion(exe) {
  const q = `(Get-Item -LiteralPath '${exe.replace(/'/g, "''")}').VersionInfo.ProductVersion`
  return execFileSync('powershell.exe', ['-NoProfile', '-Command', q], { encoding: 'utf8', windowsHide: true }).trim()
}

/** Kill only processes whose command line names OUR throwaway profile. */
function reap(profile) {
  try {
    const q =
      "Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '*" +
      profile.replace(/'/g, "''") +
      "*' } | ForEach-Object { $_.ProcessId }"
    const out = execFileSync('powershell.exe', ['-NoProfile', '-Command', q], { encoding: 'utf8', windowsHide: true })
    for (const pid of out.split(/\s+/).filter(Boolean)) {
      try {
        execFileSync('taskkill', ['/PID', pid, '/T', '/F'], { stdio: 'ignore' })
      } catch {
        /* already gone */
      }
    }
  } catch {
    /* nothing to reap */
  }
}

function pngSize(file) {
  const b = readFileSync(file)
  if (b.length < 24 || b.toString('latin1', 1, 4) !== 'PNG') throw new Error(`${file} is not a PNG`)
  return { w: b.readUInt32BE(16), h: b.readUInt32BE(20), bytes: b.length }
}

function park({ BrowserWindow }) {
  for (const w of BrowserWindow.getAllWindows()) {
    w.setOpacity(0)
    w.setPosition(-4000, -4000)
  }
}

/** Sets the fixed store (and the dirty accent when asked), reloads and opens the Settings tab. */
async function freshSettings(app, page, accent) {
  await page.evaluate(
    ([fixed, accentValue]) => {
      for (const [k, v] of Object.entries(fixed)) localStorage.setItem(k, v)
      if (accentValue) localStorage.setItem('prism.window.accent', accentValue)
      else localStorage.removeItem('prism.window.accent')
    },
    [FIXED, accent]
  )
  await page.reload()
  await page.waitForFunction(() => !!window.prism, null, { timeout: 60000 })
  await app.evaluate(park)
  await page.locator('[data-title-settings]').first().click({ force: true })
  await page.waitForSelector('[data-settings-page]', { timeout: 30000 })
  await sleep(300)
}

/** Brings one job's state about, as Flare's scene does. */
async function stage(page, job) {
  await page.locator(`[data-settings-tab=${job.page}]`).first().click({ force: true })
  await sleep(250)
  if (job.find) {
    await page.locator('[data-settings-find]').first().fill(job.find)
    await sleep(250)
  }
  switch (job.open) {
    case 'picker':
      await page.locator('[data-colour-swatch]').first().click({ force: true })
      break
    case 'select':
      await page.locator('#term-shell').first().click({ force: true })
      break
    case 'editor':
      await page.locator('[data-edit-theme]').first().click({ force: true })
      break
    case 'ask':
      // The accent is already set (a dirty theme): choosing another card asks first.
      await page.locator('[data-term-card]').nth(1).click({ force: true })
      break
    case 'help':
      await page.keyboard.press('F1')
      break
    case 'wall-expanded':
      await page.locator('[data-theme-wall-toggle]').first().click({ force: true })
      await page.evaluate((y) => {
        const pane = document.querySelector('[data-settings-page] .p-scroll')
        if (pane) pane.scrollTop = y
      }, WALL_SCROLL)
      break
    default:
      break
  }
  // The pointer rests in a corner, so no hover fill is in the picture.
  await page.mouse.move(2, 638)
  await sleep(job.open === 'help' ? 900 : 500)
}

async function main() {
  const o = parseArgs(process.argv.slice(2))
  if (o.list) {
    console.log(JSON.stringify(jobs()))
    return
  }
  const require = createRequire(join(PRISM_MODULES, 'noop.js'))
  const { _electron: electron } = require('playwright-core')
  if (!o.out) throw new Error('--out <folder> is required')
  const exe = o.exe ?? DEFAULT_EXE
  if (!existsSync(exe)) throw new Error(`Prism Terminal is not installed at ${exe}`)
  const [W, H] = o.size.split('x').map(Number)
  const wanted = o.scales ? o.scales.split(',').map(Number) : null
  const list = jobs().filter((j) => (!wanted || wanted.includes(j.scale)) && (!o.only || j.name.includes(o.only)))
  if (!list.length) throw new Error('no image matches --scales / --only')
  const out = resolve(o.out)
  mkdirSync(out, { recursive: true })

  const version = productVersion(exe)
  if (!version) throw new Error('could not read the product version')
  writeFileSync(join(out, 'VERSION.txt'), version + '\n')
  console.log(`Prism Terminal ${version}`)

  const before = REAL_PROFILES.map(stamp)
  const profile = mkdtempSync(join(tmpdir(), PROFILE_PREFIX))
  if (!resolve(profile).toLowerCase().startsWith(resolve(tmpdir()).toLowerCase())) throw new Error('profile is not under TEMP')
  const shots = []
  const problems = []
  try {
    for (const s of [...new Set(list.map((j) => j.scale))]) {
      const udd = join(profile, `s${String(s).replace('.', '_')}`)
      mkdirSync(udd)
      const app = await electron.launch({
        executablePath: exe,
        args: [`--user-data-dir=${udd}`, '--e2e', `--force-device-scale-factor=${s}`],
        env: { ...process.env }
      })
      try {
        const page = await app.firstWindow()
        await app.evaluate(park)
        await page.waitForFunction(() => !!window.prism, null, { timeout: 60000 })
        // The window adds invisible frame to a content size: ask, measure, correct until the page is W x H.
        let ask = [W, H]
        for (let k = 0; k < 4; k++) {
          await app.evaluate(({ BrowserWindow }, [w, h]) => {
            for (const win of BrowserWindow.getAllWindows()) win.setContentSize(w, h)
          }, ask)
          await sleep(300)
          const [iw, ih] = await page.evaluate(() => [innerWidth, innerHeight])
          if (iw === W && ih === H) break
          ask = [ask[0] + (W - iw), ask[1] + (H - ih)]
        }
        await app.evaluate(park)
        for (const job of list.filter((j) => j.scale === s)) {
          try {
            await freshSettings(app, page, job.open === 'ask' ? DIRTY_ACCENT : '')
            await stage(page, job)
            const file = join(out, `${job.name}.png`)
            await page.screenshot({ path: file, clip: { x: 0, y: 0, width: W, height: H } })
            shots.push({ file, s })
            console.log(`  ${job.name}`)
          } catch (e) {
            problems.push(`${job.name}: ${e?.message ?? e}`)
          }
        }
      } finally {
        await app.close().catch(() => {})
        reap(udd)
      }
    }
  } finally {
    reap(profile)
    rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 })
  }

  if (existsSync(profile)) problems.push(`profile ${profile} was not removed`)
  const after = REAL_PROFILES.map(stamp)
  REAL_PROFILES.forEach((p, i) => {
    if (before[i] !== after[i]) problems.push(`real profile touched: ${p}`)
  })
  for (const { file, s } of shots) {
    const { w, h, bytes } = pngSize(file)
    const ew = Math.round(W * s)
    const eh = Math.round(H * s)
    if (w !== ew || h !== eh) problems.push(`${file}: ${w}x${h}, wanted ${ew}x${eh}`)
    if (bytes < 8000) problems.push(`${file}: looks blank (${bytes} bytes)`)
  }
  if (problems.length) {
    for (const p of problems) console.error('FAIL ' + p)
    process.exit(1)
  }
  console.log(o.selftest ? 'ok' : `done: ${shots.length} images in ${out}`)
  if (o.selftest) rmSync(out, { recursive: true, force: true })
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((e) => {
    console.error('FAIL ' + (e?.stack ?? e))
    process.exit(1)
  })
}
