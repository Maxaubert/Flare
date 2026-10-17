// Measures Prism Terminal's Settings page (geometry and computed style) so Flare's layout can be held
// to the measure. The launch block is copied from tools/pt-shot/pt-shot.mjs: INSTALLED app, throwaway
// --user-data-dir under %TEMP%, --e2e, window parked offscreen and unfocusable. It never touches
// %APPDATA%\PrismTerminal* and never takes the foreground.
//
//   node pt-metrics.mjs [--out <json>] [--scales 1,1.5] [--exe <PrismTerminal.exe>]
//   node pt-metrics.mjs --selftest
import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'

const HERE = dirname(fileURLToPath(import.meta.url))
const PRISM_MODULES = 'C:\\Users\\Admin\\Documents\\Claude\\Github\\Prism\\node_modules'
const require = createRequire(join(PRISM_MODULES, 'noop.js'))
const { _electron: electron } = require('playwright-core')

const DEFAULT_EXE = 'C:\\Users\\Admin\\AppData\\Local\\Programs\\PrismTerminal\\PrismTerminal.exe'
const DEFAULT_OUT = join(HERE, '..', '..', 'crates', 'flare', 'tests', 'fixtures', 'pt_settings_metrics.json')
const PROFILE_PREFIX = 'pt-metrics-profile-'
const REAL_PROFILES = ['PrismTerminalStable', 'PrismTerminal'].map((n) => join(process.env.APPDATA ?? '', n))
const PROMPT_SRC = 'PS [^>]*>\\s*$'

// key -> css. A trailing "@n" takes the n-th match (default 0). "page:" searches inside
// [data-settings-page] (rects relative to it); no prefix searches the document (rects absolute).
const COMMON = {
  frame: 'self:',
  nav: 'page:nav',
  navTitle: 'page:nav h1',
  find: '[data-settings-find]',
  header: 'page:h2',
  sectionHeading: 'page:section h3',
  panel: 'page:[data-settings-panel]',
  row: 'page:[data-setting-row]',
  rowTile: 'page:[data-setting-row] > div:first-child',
  rowLabel: 'page:[data-setting-row] label',
  rowSubtext: 'page:[data-subtext]',
  rowControl: 'page:[data-row-control]'
}
const TABS = ['appearance', 'terminal', 'agents', 'dictation', 'diagnostics', 'about']
const PAGE_EXTRA = {
  appearance: {
    segmented: 'page:[data-seg]',
    saveTerm: 'page:[data-save-term]',
    slider: 'page:input[type=range]',
    sliderValue: 'page:[data-slider-value]',
    colourCode: 'page:[data-row-control] input[aria-label]',
    colourSwatch: 'page:[data-colour-swatch]',
    termCard: 'page:[data-term-card]',
    termCardLabel: 'page:[data-term-card] > div:last-child',
    wallToggle: 'page:[data-theme-wall-toggle]',
    wall: 'page:[data-term-wall]'
  },
  terminal: {
    switchOne: 'page:[role=switch]',
    switchKnob: 'page:[role=switch] span',
    selectTrigger: 'page:button[aria-haspopup=listbox]',
    segmented: 'page:[data-seg]'
  },
  agents: { switchOne: 'page:[role=switch]' },
  dictation: {},
  diagnostics: { switchOne: 'page:[role=switch]', buttonOne: 'page:[data-row-control] button@0', buttonTwo: 'page:[data-row-control] button@1' },
  about: { version: 'page:[data-app-version]', aboutHeading: 'page:h4' }
}

function parseArgs(argv) {
  const o = { scales: '1,1.5', selftest: false }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--selftest') o.selftest = true
    else if (a.startsWith('--')) o[a.slice(2)] = argv[++i]
  }
  if (o.selftest) o.scales = '1'
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

function park({ BrowserWindow }) {
  for (const w of BrowserWindow.getAllWindows()) {
    w.setOpacity(0)
    w.setPosition(-4000, -4000)
  }
}

/** Runs in the page: measures every spec (see the key notes above). */
function measureAll(specs) {
  const page = document.querySelector('[data-settings-page]')
  const out = {}
  const missing = []
  const base = page ? page.getBoundingClientRect() : { left: 0, top: 0 }
  const r2 = (n) => Math.round(n * 100) / 100
  const PROPS = [
    'fontSize', 'fontWeight', 'lineHeight', 'color', 'backgroundColor', 'borderRadius', 'borderTopWidth',
    'gap', 'paddingLeft', 'paddingRight', 'paddingTop', 'paddingBottom'
  ]
  for (const [key, raw] of Object.entries(specs)) {
    let css = raw
    let scope = 'doc'
    if (css.startsWith('page:')) {
      scope = 'page'
      css = css.slice(5)
    } else if (css.startsWith('self:')) {
      scope = 'self'
      css = ''
    }
    let nth = 0
    const m = /@(\d+)$/.exec(css)
    if (m) {
      nth = Number(m[1])
      css = css.slice(0, m.index)
    }
    let el = null
    if (scope === 'self') el = page
    else {
      const root = scope === 'page' ? page : document
      if (root) {
        const mm = /^(.*):has-text\("(.*)"\)$/.exec(css)
        if (mm) el = [...root.querySelectorAll(mm[1])].filter((e) => e.textContent.includes(mm[2]))[nth]
        else el = root.querySelectorAll(css)[nth]
      }
    }
    if (!el) {
      missing.push(key)
      continue
    }
    const b = el.getBoundingClientRect()
    const cs = getComputedStyle(el)
    const style = {}
    for (const p of PROPS) style[p] = cs[p]
    const ox = scope === 'page' || scope === 'self' ? base.left : 0
    const oy = scope === 'page' || scope === 'self' ? base.top : 0
    out[key] = { x: r2(b.left - ox), y: r2(b.top - oy), width: r2(b.width), height: r2(b.height), style }
  }
  return { out, missing }
}

async function waitPrompt(page) {
  const welcome = page.getByText('New terminal', { exact: true }).first()
  if (!(await page.locator('.xterm').count()) && (await welcome.count())) await welcome.click({ force: true })
  await page.waitForFunction(
    (src) => new RegExp(src).test((document.querySelector('.xterm .xterm-rows')?.textContent ?? '').trimEnd()),
    PROMPT_SRC,
    { timeout: 60000 }
  )
}

async function capture(app, page, scale, selftest, problems) {
  const result = {}
  const grab = async (where, specs) => {
    const { out, missing } = await page.evaluate(measureAll, specs)
    for (const k of missing) problems.push(`scale ${scale}: ${where}: selector not found: ${k}`)
    return out
  }

  await page.locator('[data-title-settings]').first().click({ force: true })
  await page.waitForSelector('[data-settings-page]', { timeout: 30000 })
  await app.evaluate(park)

  const pages = selftest ? ['appearance'] : TABS
  for (const id of pages) {
    await page.locator(`[data-settings-tab=${id}]`).first().click({ force: true })
    await sleep(250)
    const specs = { ...COMMON, ...PAGE_EXTRA[id], tab: `page:[data-settings-tab=${id}]` }
    // Diagnostics and About have no section heading (sectionIds: the page title says it).
    if (id === 'diagnostics' || id === 'about') delete specs.sectionHeading
    if (id === 'appearance') for (const t of TABS) specs[`tab_${t}`] = `page:[data-settings-tab=${t}]`
    result[id] = await grab(id, specs)
  }
  if (selftest) return result

  // Popups, each with its own key.
  await page.locator('[data-settings-tab=appearance]').first().click({ force: true })
  await sleep(250)
  await page.locator('[data-colour-swatch]').first().click({ force: true })
  await sleep(250)
  result.colourPopover = await grab('colour popover', {
    popover: '[data-colour-popover]',
    slider0: '[data-colour-popover] [role=slider]@0',
    slider1: '[data-colour-popover] [role=slider]@1',
    eyedropper: '[data-colour-eyedropper]',
    format: '[data-colour-format]',
    square: '[data-colour-popover] > div:first-child'
  })
  await page.keyboard.press('Escape')
  await sleep(200)

  await page.locator('[data-settings-tab=terminal]').first().click({ force: true })
  await sleep(250)
  await page.locator('#term-shell').first().click({ force: true })
  await sleep(250)
  result.shellSelect = await grab('shell select', { listbox: '[role=listbox]', option: '[role=option]' })
  await page.keyboard.press('Escape')
  await sleep(200)

  await page.locator('[data-settings-tab=appearance]').first().click({ force: true })
  await sleep(250)
  await page.locator('[data-edit-theme]').first().click({ force: true })
  await sleep(300)
  result.themeEditor = await grab('theme editor', {
    editor: '[data-theme-editor]',
    modal: '[data-modal]',
    saveCustom: '[data-save-custom]'
  })
  await page.keyboard.press('Escape')
  await sleep(200)

  await page.keyboard.press('F1')
  await sleep(500)
  result.helpPanel = await grab('help panel', {
    panel: '[data-help-panel]',
    search: '[data-help-search]',
    shell0: '[data-help-shell]@0',
    shell1: '[data-help-shell]@1',
    shell2: '[data-help-shell]@2',
    count: '[data-help-count]',
    row: '[data-help-row]',
    task: '[data-help-row] [data-help-task]',
    well: '[data-help-row] [data-help-well]',
    copy: '[data-help-row] [data-help-copy]',
    category: '[data-help-category]',
    footer: '[data-help-panel] > :last-child'
  })
  await page.keyboard.press('Escape')
  return result
}

async function main() {
  const o = parseArgs(process.argv.slice(2))
  const exe = o.exe ?? DEFAULT_EXE
  if (!existsSync(exe)) throw new Error(`Prism Terminal is not installed at ${exe}`)
  const outFile = resolve(o.out ?? DEFAULT_OUT)
  const scales = o.scales.split(',').map(Number)
  const [W, H] = [1000, 640]
  const version = productVersion(exe)
  if (!version) throw new Error('could not read the product version')
  console.log(`Prism Terminal ${version}`)

  const before = REAL_PROFILES.map(stamp)
  const profile = mkdtempSync(join(tmpdir(), PROFILE_PREFIX))
  if (!resolve(profile).toLowerCase().startsWith(resolve(tmpdir()).toLowerCase())) throw new Error('profile is not under TEMP')
  const problems = []
  const scalesOut = {}
  try {
    for (const s of scales) {
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
        await waitPrompt(page).catch(() => {})
        scalesOut[String(s)] = await capture(app, page, s, o.selftest, problems)
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

  // Self-checks at scale 1.
  const s1 = scalesOut['1']
  const ap = s1?.appearance
  const near = (a, b) => typeof a === 'number' && Math.abs(a - b) <= 0.5
  if (ap) {
    if (!near(ap.nav?.width, 244)) problems.push(`rail is ${ap.nav?.width} wide, wanted 244`)
    if (!(ap.row?.height >= 58)) problems.push(`row is ${ap.row?.height} high, wanted at least 58`)
    if (!near(ap.rowTile?.width, 32) || !near(ap.rowTile?.height, 32))
      problems.push(`tile is ${ap.rowTile?.width}x${ap.rowTile?.height}, wanted 32x32`)
  } else problems.push('no scale 1 appearance measurements')
  if (!o.selftest) {
    const sw = s1?.terminal?.switchOne
    if (!near(sw?.width, 36) || !near(sw?.height, 20)) problems.push(`switch is ${sw?.width}x${sw?.height}, wanted 36x20`)
    if (!near(s1?.colourPopover?.popover?.width, 226)) problems.push(`popover is ${s1?.colourPopover?.popover?.width} wide, wanted 226`)
    if (!near(s1?.helpPanel?.panel?.width, 620)) problems.push(`help panel is ${s1?.helpPanel?.panel?.width} wide, wanted 620`)
  }

  if (problems.length) {
    for (const p of problems) console.error('FAIL ' + p)
    process.exit(1)
  }
  if (o.selftest) {
    console.log('ok')
    return
  }
  mkdirSync(dirname(outFile), { recursive: true })
  writeFileSync(outFile, JSON.stringify({ pt_version: version, scales: scalesOut }, null, 2) + '\n')
  console.log(`done: ${outFile}`)
}

main().catch((e) => {
  console.error('FAIL ' + (e?.stack ?? e))
  process.exit(1)
})
