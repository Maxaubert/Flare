// Measures Prism Terminal's update chip, the notice under it and the update window (geometry and
// computed style) at several display scales, so Flare's chip and window are held to the measure
// (M9 plan, Task 3, Appendix C). Same launch block as pt-metrics.mjs: the INSTALLED app, a throwaway
// --user-data-dir under %TEMP%, --e2e, the window parked offscreen and unfocusable. It runs with
// --preview-update (the fake offer: no network, nothing downloaded, nothing installed, no quit). It
// never touches %APPDATA%\PrismTerminal* and never takes the foreground.
//
//   node update.mjs [--out <json>] [--shots <dir>] [--scales 1,1.5,2.25] [--exe <PrismTerminal.exe>]
//   node update.mjs --selftest      (scale 1, no screenshots, nothing written)
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
const DEFAULT_OUT = join(HERE, 'update.json')
const DEFAULT_SHOTS = join(HERE, '..', '..', 'docs', 'screenshots', 'm9', 'pt')
const PROFILE_PREFIX = 'pt-update-profile-'
const REAL_PROFILES = ['PrismTerminalStable', 'PrismTerminal'].map((n) => join(process.env.APPDATA ?? '', n))
const NOTICE_TEXT = 'Preview only: nothing was installed'

function parseArgs(argv) {
  const o = { scales: '1,1.5,2.25', selftest: false }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--selftest') o.selftest = true
    else if (a.startsWith('--')) o[a.slice(2)] = argv[++i]
  }
  if (o.selftest) o.scales = '1'
  return o
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const stamp = (p) => {
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

// key -> css (document scope, rects absolute in css px). "@n" takes the n-th match.
const CHIP = {
  chip: '[data-update-chip]',
  chipLabel: '[data-update-chip] [data-update-label]',
  chipSvg: '[data-update-chip] svg',
  chipPath: '[data-update-chip] svg path'
}
const DIALOG = {
  scrim: '[data-update-scrim]',
  dialog: '[data-update-dialog]',
  h2: '[data-update-dialog] h2',
  current: '[data-update-current]',
  notes: '[data-update-notes]',
  sectionH3: '[data-update-section] h3',
  section: '[data-update-section]',
  entry: '[data-update-entry]',
  entryDot: '[data-update-entry] > span:first-child',
  entryText: '[data-update-entry] > span:last-child',
  progress: '[data-update-progress]',
  status: '[data-update-status]',
  pct: '[data-update-pct]',
  track: '[data-update-track]',
  fill: '[data-update-fill]',
  cancel: '[data-update-cancel]',
  install: '[data-update-install]'
}
const MORE = { more: '[data-update-more]' }
const NOTICE = { notice: '[data-update-notice]' }

/**
 * Runs in the page. Measures every spec, optionally after waiting for the preview install to reach a
 * percentage (`wait.pct`, polled every 10 ms) so the numbers belong to one instant.
 */
async function measureWhen({ specs, wait, extras }) {
  const sleepIn = (ms) => new Promise((r) => setTimeout(r, ms))
  const deadline = Date.now() + 15000
  const pctNow = () => Number(document.querySelector('[data-update-track]')?.getAttribute('aria-valuenow') ?? -1)
  const phaseNow = () => document.querySelector('[data-update-dialog]')?.dataset.phase ?? 'gone'
  if (wait?.pct !== undefined) {
    while (!(pctNow() >= wait.pct && phaseNow() !== 'idle') && Date.now() < deadline) await sleepIn(10)
    // The fill animates its width over 300 ms: let it land (the 100 percent hold is 600 ms).
    if (wait.settle) await sleepIn(wait.settle)
  }
  const r2 = (n) => Math.round(n * 100) / 100
  const PROPS = [
    'fontFamily', 'fontSize', 'fontWeight', 'lineHeight', 'letterSpacing', 'color', 'backgroundColor', 'borderRadius',
    'borderTopWidth', 'borderTopColor', 'borderTopStyle', 'boxShadow', 'gap', 'columnGap', 'rowGap', 'display',
    'flexDirection', 'alignItems', 'justifyContent', 'paddingLeft', 'paddingRight', 'paddingTop', 'paddingBottom',
    'marginTop', 'maxWidth', 'maxHeight', 'minHeight', 'overflowY', 'opacity', 'visibility', 'transition', 'backdropFilter',
    'stroke', 'strokeWidth', 'strokeLinecap', 'strokeLinejoin', 'whiteSpace'
  ]
  const out = {}
  const missing = []
  for (const [key, raw] of Object.entries(specs)) {
    let css = raw
    let nth = 0
    const m = /@(\d+)$/.exec(css)
    if (m) {
      nth = Number(m[1])
      css = css.slice(0, m.index)
    }
    const el = document.querySelectorAll(css)[nth]
    if (!el) {
      missing.push(key)
      continue
    }
    const b = el.getBoundingClientRect()
    const cs = getComputedStyle(el)
    const style = {}
    for (const p of PROPS) style[p] = cs[p]
    const rec = { x: r2(b.left), y: r2(b.top), width: r2(b.width), height: r2(b.height), style }
    rec.scrollHeight = el.scrollHeight
    rec.clientHeight = el.clientHeight
    if (el.style && el.style.width) rec.inlineWidth = el.style.width
    if (el.textContent && el.children.length === 0) rec.text = el.textContent.trim()
    for (const a of ['data-phase', 'data-active', 'aria-valuenow', 'aria-label', 'title', 'role']) {
      const v = el.getAttribute(a)
      if (v !== null) (rec.attrs ??= {})[a] = v
    }
    if (el.disabled !== undefined) rec.disabled = el.disabled
    out[key] = rec
  }
  const result = { out, missing, viewport: { width: innerWidth, height: innerHeight }, pct: pctNow(), phase: phaseNow() }
  if (extras?.includes('chipGroup')) {
    const chip = document.querySelector('[data-update-chip]')
    if (chip) {
      const c = chip.getBoundingClientRect()
      const others = [...document.querySelectorAll('[data-title-bar] button')]
        .filter((bt) => bt !== chip && bt.getBoundingClientRect().left >= c.right - 0.5)
        .map((bt) => bt.getBoundingClientRect())
        .sort((a, z) => a.left - z.left)
      const bar = document.querySelector('[data-title-bar]')?.getBoundingClientRect()
      result.chipLayout = {
        toRightEdge: r2(innerWidth - c.right),
        toNextControl: others.length ? r2(others[0].left - c.right) : null,
        controlsRight: others.length,
        lastControlToRightEdge: others.length ? r2(innerWidth - others[others.length - 1].right) : null,
        titleBar: bar ? { x: r2(bar.left), y: r2(bar.top), width: r2(bar.width), height: r2(bar.height) } : null,
        chipCentreY: r2(c.top + c.height / 2),
        barCentreY: bar ? r2(bar.top + bar.height / 2) : null
      }
      const notice = document.querySelector('[data-update-notice]')
      if (notice) {
        const n = notice.getBoundingClientRect()
        result.chipLayout.noticeRightToChipRight = r2(c.right - n.right)
        result.chipLayout.noticeTopBelowChip = r2(n.top - c.bottom)
      }
    }
  }
  if (extras?.includes('dialogLayout')) {
    const d = document.querySelector('[data-update-dialog]')?.getBoundingClientRect()
    const pick = (sel) => document.querySelector(sel)?.getBoundingClientRect()
    if (d) {
      const gap = (a, bSel) => {
        const x = pick(a)
        const y = pick(bSel)
        return x && y ? r2(y.top - x.bottom) : null
      }
      const cancel = pick('[data-update-cancel]')
      const install = pick('[data-update-install]')
      result.dialogLayout = {
        centreX: r2(d.left + d.width / 2 - innerWidth / 2),
        centreY: r2(d.top + d.height / 2 - innerHeight / 2),
        h2ToCurrent: gap('[data-update-dialog] h2', '[data-update-current]'),
        currentToNotes: gap('[data-update-current]', '[data-update-notes]'),
        notesToProgress: gap('[data-update-notes]', '[data-update-progress]'),
        progressToButtons: gap('[data-update-progress]', '[data-update-cancel]'),
        statusToTrack: gap('[data-update-status]', '[data-update-track]'),
        buttonsToBottom: cancel ? r2(d.bottom - cancel.bottom) : null,
        installToRight: install ? r2(d.right - install.right) : null,
        cancelToInstall: cancel && install ? r2(install.left - cancel.right) : null
      }
    }
  }
  return result
}

/** The window's size is asked for as CONTENT size and corrected until the page is W x H. */
async function setContent(app, page, W, H) {
  let ask = [W, H]
  for (let k = 0; k < 5; k++) {
    await app.evaluate(({ BrowserWindow }, [w, h]) => {
      for (const win of BrowserWindow.getAllWindows()) win.setContentSize(w, h)
    }, ask)
    await sleep(350)
    const [iw, ih] = await page.evaluate(() => [innerWidth, innerHeight])
    if (iw === W && ih === H) return true
    ask = [ask[0] + (W - iw), ask[1] + (H - ih)]
  }
  return false
}

/** n pull-request lines in the shape `gh release create --generate-notes` writes. */
function manyNotes(n) {
  const kinds = [
    'Dictation: speak into the terminal, transcribed on your own machine',
    'fix(terminal): a prompt survives the window getting narrower and wider again',
    'The update button opens a window with the patch notes, Cancel and Install',
    'Links are painted in a colour that follows the theme',
    'fix(tabs): closing a tab that hosts an agent asks first, working or idle',
    'Bump electron from 43.1.0 to 43.1.1',
    'feat: Explorer entries read "Open terminal here", and a window edges setting'
  ]
  const lines = ['<!-- Release notes generated using configuration in .github/release.yml at main -->', '', "## What's Changed"]
  for (let i = 0; i < n; i++) {
    lines.push(`* ${kinds[i % kinds.length]} (#${100 + i}) by @Maxaubert in https://github.com/Maxaubert/PrismTerminal/pull/${100 + i}`)
  }
  lines.push('', '**Full Changelog**: https://github.com/Maxaubert/PrismTerminal/compare/v0.4.0...v0.5.0')
  return lines.join('\n')
}

let exePath = DEFAULT_EXE

async function launch(udd, s, extraArgs, env) {
  mkdirSync(udd, { recursive: true })
  const app = await electron.launch({
    executablePath: exePath,
    args: [`--user-data-dir=${udd}`, '--e2e', `--force-device-scale-factor=${s}`, ...extraArgs],
    env: { ...process.env, ...env }
  })
  const page = await app.firstWindow()
  await app.evaluate(park)
  await page.waitForFunction(() => !!window.prism, null, { timeout: 60000 })
  return { app, page }
}

async function captureScale(s, profile, shotsDir, selftest, problems) {
  const tag = String(s).replace('.', '_')
  const res = {}
  const grab = async (page, where, specs, wait, extras) => {
    const r = await page.evaluate(measureWhen, { specs, wait, extras })
    for (const k of r.missing) problems.push(`scale ${s}: ${where}: selector not found: ${k}`)
    return r
  }
  const shot = async (page, name, clip) => {
    if (selftest || !shotsDir) return
    await page.screenshot({ path: join(shotsDir, `${name}-${tag}.png`), ...(clip ? { clip } : {}) })
  }

  // ---- Part 1: the preview offer (chip, window, progress, ending) ----
  {
    const udd = join(profile, `a${tag}`)
    const { app, page } = await launch(udd, s, ['--preview-update'], {})
    try {
      const W = 1000
      if (!(await setContent(app, page, W, 640))) problems.push(`scale ${s}: could not size the window to 1000x640`)
      await app.evaluate(park)
      await page.waitForSelector('[data-update-chip]', { timeout: 30000 })
      await sleep(400)
      const chipIdle = await grab(page, 'chip', CHIP, undefined, ['chipGroup'])
      res.chip = chipIdle.out
      res.chipLayout = chipIdle.chipLayout
      res.viewport = chipIdle.viewport
      await shot(page, 'chip', { x: Math.max(0, chipIdle.viewport.width - 400), y: 0, width: Math.min(400, chipIdle.viewport.width), height: 44 })

      await page.locator('[data-update-chip]').first().click({ force: true })
      await page.waitForSelector('[data-update-dialog]', { timeout: 10000 })
      await sleep(500)
      const idle = await grab(page, 'window idle', DIALOG, undefined, ['dialogLayout'])
      res.window = { height640: idle.out, layout640: idle.dialogLayout, viewport640: idle.viewport }
      await shot(page, 'window')

      // The smallest-window rule: the notes' height is capped at min(44vh, 300px) at 420.
      await page.keyboard.press('Escape')
      await sleep(300)
      if (!(await setContent(app, page, W, 420))) problems.push(`scale ${s}: could not size the window to 1000x420`)
      await app.evaluate(park)
      await sleep(300)
      await page.locator('[data-update-chip]').first().click({ force: true })
      await page.waitForSelector('[data-update-dialog]', { timeout: 10000 })
      await sleep(500)
      const small = await grab(page, 'window 420', DIALOG, undefined, ['dialogLayout'])
      res.window.height420 = small.out
      res.window.layout420 = small.dialogLayout
      res.window.viewport420 = small.viewport
      await page.keyboard.press('Escape')
      await sleep(300)
      await setContent(app, page, W, 640)
      await app.evaluate(park)
      await sleep(300)

      // The install, sampled at 0, 50 and 100 percent.
      await page.locator('[data-update-chip]').first().click({ force: true })
      await page.waitForSelector('[data-update-dialog]', { timeout: 10000 })
      await sleep(400)
      await page.locator('[data-update-install]').first().click({ force: true })
      res.running = {}
      const at0 = await grab(page, 'progress 0', DIALOG, { pct: 0 }, ['dialogLayout'])
      res.running.pct0 = { measured_pct: at0.pct, phase: at0.phase, ...at0.out }
      const at50 = await grab(page, 'progress 50', DIALOG, { pct: 50 }, ['dialogLayout'])
      res.running.pct50 = { measured_pct: at50.pct, phase: at50.phase, ...at50.out }
      await shot(page, 'window-running')
      const at100 = await grab(page, 'progress 100', DIALOG, { pct: 100, settle: 350 }, ['dialogLayout'])
      res.running.pct100 = { measured_pct: at100.pct, phase: at100.phase, ...at100.out }
      res.running.chipDuring = (await grab(page, 'chip while running', CHIP, undefined, ['chipGroup'])).out

      // The ending, said IN the window.
      await page.waitForFunction(
        (t) =>
          document.querySelector('[data-update-dialog]')?.dataset.phase === 'idle' &&
          (document.querySelector('[data-update-status]')?.textContent ?? '').trim() === t,
        NOTICE_TEXT,
        { timeout: 15000, polling: 25 }
      )
      await sleep(350)
      const ended = await grab(page, 'window notice', DIALOG, undefined, ['dialogLayout'])
      res.windowNotice = { ...ended.out }
      await shot(page, 'window-notice')

      // The notice under the chip. The app puts it there only when an install ends with the window
      // away (a host question hid it), which the preview cannot reach. So the SAME markup and classes
      // are inserted into the chip's wrapper and measured: the geometry is the real stylesheet's.
      await page.keyboard.press('Escape')
      await sleep(400)
      const injected = await page.evaluate((text) => {
        const chip = document.querySelector('[data-update-chip]')
        if (!chip || !chip.parentElement) return false
        const d = document.createElement('div')
        d.setAttribute('data-update-notice', '')
        d.setAttribute('role', 'status')
        d.className =
          'absolute right-0 top-[calc(100%+8px)] z-40 whitespace-nowrap rounded-md border border-[color:var(--p-divider)] bg-[var(--p-side-flat)] px-2.5 py-1.5 text-[11.5px] text-[var(--p-text-soft)] shadow-[0_10px_30px_rgba(0,0,0,.45)]'
        d.textContent = text
        chip.parentElement.appendChild(d)
        return true
      }, NOTICE_TEXT)
      if (!injected) problems.push(`scale ${s}: could not place the notice markup`)
      await sleep(200)
      const nt = await grab(page, 'notice', { ...NOTICE, ...CHIP }, undefined, ['chipGroup'])
      res.notice = { source: 'same markup and classes as UpdateChip.tsx, inserted by the script', ...nt.out, layout: nt.chipLayout }
      await shot(page, 'chip-notice', { x: Math.max(0, nt.viewport.width - 400), y: 0, width: Math.min(400, nt.viewport.width), height: 90 })
    } finally {
      await app.close().catch(() => {})
      reap(udd)
    }
  }

  // ---- Part 2: a real-shaped offer with 25 entries (the "+ N more" line) ----
  if (!selftest) {
    const udd = join(profile, `b${tag}`)
    const { app, page } = await launch(udd, s, [], { PT_E2E_UPDATE_OFFER: 'https://example.invalid/none.exe', PT_E2E_UPDATE_NOTES: manyNotes(25) })
    try {
      await setContent(app, page, 1000, 640)
      await app.evaluate(park)
      await page.waitForSelector('[data-update-chip]', { timeout: 30000 })
      await sleep(400)
      await page.locator('[data-update-chip]').first().click({ force: true })
      await page.waitForSelector('[data-update-dialog]', { timeout: 10000 })
      await sleep(500)
      const many = await grab(page, 'many entries', { ...DIALOG, ...MORE }, undefined, ['dialogLayout'])
      const entries = await page.locator('[data-update-entry]').count()
      res.manyEntries = { entryCount: entries, ...many.out, layout: many.dialogLayout }
      await shot(page, 'window-many')
    } finally {
      await app.close().catch(() => {})
      reap(udd)
    }
  }
  return res
}

async function main() {
  const o = parseArgs(process.argv.slice(2))
  exePath = o.exe ?? DEFAULT_EXE
  if (!existsSync(exePath)) throw new Error(`Prism Terminal is not installed at ${exePath}`)
  const outFile = resolve(o.out ?? DEFAULT_OUT)
  const shotsDir = o.selftest ? null : resolve(o.shots ?? DEFAULT_SHOTS)
  if (shotsDir) mkdirSync(shotsDir, { recursive: true })
  const scales = o.scales.split(',').map(Number)
  const version = productVersion(exePath)
  if (!version) throw new Error('could not read the product version')
  console.log(`Prism Terminal ${version}`)

  const before = REAL_PROFILES.map(stamp)
  const profile = mkdtempSync(join(tmpdir(), PROFILE_PREFIX))
  if (!resolve(profile).toLowerCase().startsWith(resolve(tmpdir()).toLowerCase())) throw new Error('profile is not under TEMP')
  const problems = []
  const scalesOut = {}
  try {
    for (const s of scales) {
      scalesOut[String(s)] = await captureScale(s, profile, shotsDir, o.selftest, problems)
      console.log(`scale ${s} done`)
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

  // Self-checks at scale 1 against Appendix C.
  const s1 = scalesOut['1']
  const near = (a, b) => typeof a === 'number' && Math.abs(a - b) <= 0.5
  if (s1) {
    if (!near(s1.chip?.chip?.height, 24)) problems.push(`chip is ${s1.chip?.chip?.height} high, wanted 24`)
    const d = s1.window?.height640?.dialog
    if (!near(d?.width, 460)) problems.push(`window is ${d?.width} wide, wanted 460`)
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
  writeFileSync(
    outFile,
    JSON.stringify({ pt_version: version, window_content_px: { tall: [1000, 640], short: [1000, 420] }, scales: scalesOut }, null, 2) + '\n'
  )
  console.log(`done: ${outFile}`)
}

main().catch((e) => {
  console.error('FAIL ' + (e?.stack ?? e))
  process.exit(1)
})
