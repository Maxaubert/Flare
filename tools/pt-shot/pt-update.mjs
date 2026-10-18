// Screenshots the installed Prism Terminal's update chip, notice and window in the eight states of
// Flare's update scenes (M9 plan, Task 40), at the same display scales, so the side-by-sides pair up
// by name: <scene>-<scale>.png with scene = update-chip, update-chip-notice, update-window,
// update-window-running, update-window-installing, update-window-notice, update-window-small,
// update-window-empty, and scale tag 1, 1_5 or 2_25.
//
//   node pt-update.mjs [--out <dir>] [--scales 1,1.5,2.25] [--exe <PrismTerminal.exe>] [--only <text>]
//   node pt-update.mjs --selftest      (scale 1, no images written)
//
// Same launch block as tools/pt-metrics/update.mjs: the INSTALLED app, a throwaway --user-data-dir
// under %TEMP%, --e2e, the window parked offscreen (opacity 0, at -4000,-4000) and unfocusable. The
// first six states run with --preview-update (a fake offer: no network, nothing downloaded or
// installed). The last two (25 entries at 1000x420, and no notes) run on the e2e offer variables of
// the app, because the preview has neither. It never touches %APPDATA%\PrismTerminal* and never
// takes the foreground.
//
// The 42 percent frame. The preview install runs in the app's main process on real timers, so the
// page's clock cannot pause it (page.clock only fakes the page's own timers). The preview reports
// 40 and then 45 percent, 120 ms apart. The script waits for 40, freezes the bar at 42 (width and
// label, transitions off) and takes the picture at once; it checks the page has not passed 45 and
// tries again (a fresh launch) up to three times when it has.
import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'

const HERE = dirname(fileURLToPath(import.meta.url))
const PRISM_MODULES = 'C:\\Users\\Admin\\Documents\\Claude\\Github\\Prism\\node_modules'
const require = createRequire(join(PRISM_MODULES, 'noop.js'))
const { _electron: electron } = require('playwright-core')

const DEFAULT_EXE = 'C:\\Users\\Admin\\AppData\\Local\\Programs\\PrismTerminal\\PrismTerminal.exe'
const DEFAULT_OUT = join(HERE, '..', '..', 'docs', 'screenshots', 'm9', 'pt')
const PROFILE_PREFIX = 'pt-update-shots-'
const REAL_PROFILES = ['PrismTerminalStable', 'PrismTerminal'].map((n) => join(process.env.APPDATA ?? '', n))
const NOTICE_TEXT = 'Preview only: nothing was installed'
const NO_NOTES_TEXT = 'No notes were published with this release.'
const TALL = [1000, 640]
const SHORT = [1000, 420]

const SCENES = [
  'update-chip',
  'update-chip-notice',
  'update-window',
  'update-window-running',
  'update-window-installing',
  'update-window-notice',
  'update-window-small',
  'update-window-empty'
]

function parseArgs(argv) {
  const o = { scales: '1,1.5,2.25', selftest: false, only: '' }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--selftest') o.selftest = true
    else if (a === '--list') o.list = true
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

/** The window's size is asked for as CONTENT size and corrected until the page is W x H. */
async function setContent(app, page, [W, H]) {
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
  const lines = ['## What\'s Changed']
  for (let i = 0; i < n; i++) {
    lines.push(`* ${kinds[i % kinds.length]} (#${100 + i}) by @Maxaubert in https://github.com/Maxaubert/PrismTerminal/pull/${100 + i}`)
  }
  return lines.join('\n')
}

const pctNow = () => Number(document.querySelector('[data-update-track]')?.getAttribute('aria-valuenow') ?? -1)

/** Reads the PNG's size from its header. */
function pngSize(file) {
  const b = readFileSync(file)
  return [b.readUInt32BE(16), b.readUInt32BE(20)]
}

async function openWindow(page) {
  await page.waitForSelector('[data-update-chip]', { timeout: 30000 })
  await sleep(400)
  await page.locator('[data-update-chip]').first().click({ force: true })
  await page.waitForSelector('[data-update-dialog]', { timeout: 10000 })
  await sleep(500)
}

async function waitPct(page, pct) {
  await page.waitForFunction(
    (p) => {
      const n = Number(document.querySelector('[data-update-track]')?.getAttribute('aria-valuenow') ?? -1)
      return n >= p && document.querySelector('[data-update-dialog]')?.dataset.phase !== 'idle'
    },
    pct,
    { timeout: 15000, polling: 10 }
  )
}

/**
 * One scale: every state, each in its own launch of the app so no state leaks into the next.
 * `want` filters the scenes (the --only text).
 */
async function captureScale(s, profile, out, selftest, want, problems) {
  const tag = String(s).replace('.', '_')
  let n = 0
  const shot = async (page, scene, size) => {
    if (selftest) return
    const file = join(out, `${scene}-${tag}.png`)
    await page.screenshot({ path: file })
    const [w, h] = pngSize(file)
    const ew = Math.round(size[0] * s)
    const eh = Math.round(size[1] * s)
    if (w !== ew || h !== eh) problems.push(`${scene} at ${s}: image is ${w}x${h}, wanted ${ew}x${eh}`)
    n++
  }
  const run = async (scene, args, env, size, body) => {
    if (want && !scene.includes(want)) return
    const udd = join(profile, `${scene}-${tag}`)
    const { app, page } = await launch(udd, s, args, env)
    try {
      if (!(await setContent(app, page, size))) problems.push(`${scene} at ${s}: could not size the window to ${size.join('x')}`)
      await app.evaluate(park)
      await body(page)
      await shot(page, scene, size)
    } finally {
      await app.close().catch(() => {})
      reap(udd)
    }
  }
  const preview = ['--preview-update']

  await run('update-chip', preview, {}, TALL, async (page) => {
    await page.waitForSelector('[data-update-chip]', { timeout: 30000 })
    await sleep(400)
  })

  await run('update-chip-notice', preview, {}, TALL, async (page) => {
    await page.waitForSelector('[data-update-chip]', { timeout: 30000 })
    await sleep(400)
    // The app puts the notice under the chip only when an install ends with the window away (a host
    // question hid it), which the preview cannot reach. The SAME markup and classes as UpdateChip.tsx
    // are inserted into the chip's wrapper, so the geometry is the real stylesheet's. The text is the
    // failure Flare's scene shows.
    const ok = await page.evaluate((text) => {
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
    }, 'The update could not be downloaded. Nothing was installed.')
    if (!ok) problems.push(`update-chip-notice at ${s}: could not place the notice markup`)
    await sleep(200)
  })

  await run('update-window', preview, {}, TALL, openWindow)

  // The running frame: up to three fresh launches until the picture is taken at 40 to 45 percent.
  if (!want || 'update-window-running'.includes(want)) {
    const scene = 'update-window-running'
    let done = false
    for (let attempt = 0; attempt < 3 && !done; attempt++) {
      const udd = join(profile, `${scene}-${tag}-${attempt}`)
      const { app, page } = await launch(udd, s, preview, {})
      try {
        await setContent(app, page, TALL)
        await app.evaluate(park)
        await openWindow(page)
        await page.locator('[data-update-install]').first().click({ force: true })
        await waitPct(page, 40)
        await page.addStyleTag({ content: '[data-update-fill]{transition:none!important;width:42%!important}' })
        await page.evaluate(() => {
          const pct = document.querySelector('[data-update-pct]')
          if (pct) pct.textContent = '42%'
        })
        if (!selftest) await page.screenshot({ path: join(out, `${scene}-${tag}.png`) })
        const after = await page.evaluate(pctNow)
        if (after >= 0 && after <= 45) {
          done = true
          if (!selftest) {
            const [w, h] = pngSize(join(out, `${scene}-${tag}.png`))
            if (w !== Math.round(TALL[0] * s) || h !== Math.round(TALL[1] * s)) problems.push(`${scene} at ${s}: image is ${w}x${h}`)
            n++
          }
        }
      } finally {
        await app.close().catch(() => {})
        reap(udd)
      }
    }
    if (!done) problems.push(`${scene} at ${s}: the bar had passed 45 percent in all three tries`)
  }

  await run('update-window-installing', preview, {}, TALL, async (page) => {
    await openWindow(page)
    await page.locator('[data-update-install]').first().click({ force: true })
    // The fill animates its width over 300 ms and the 100 percent hold is 600 ms.
    await waitPct(page, 100)
    await sleep(350)
  })

  await run('update-window-notice', preview, {}, TALL, async (page) => {
    await openWindow(page)
    await page.locator('[data-update-install]').first().click({ force: true })
    await page.waitForFunction(
      (t) =>
        document.querySelector('[data-update-dialog]')?.dataset.phase === 'idle' &&
        (document.querySelector('[data-update-status]')?.textContent ?? '').trim() === t,
      NOTICE_TEXT,
      { timeout: 15000, polling: 25 }
    )
    await sleep(350)
  })

  // 25 entries in a 1000x420 window: the real-shaped offer of the app's e2e variables.
  await run(
    'update-window-small',
    [],
    { PT_E2E_UPDATE_OFFER: 'https://example.invalid/none.exe', PT_E2E_UPDATE_NOTES: manyNotes(25) },
    SHORT,
    async (page) => {
      await openWindow(page)
      const entries = await page.locator('[data-update-entry]').count()
      if (entries !== 20) problems.push(`update-window-small at ${s}: ${entries} entries shown, wanted 20`)
    }
  )

  // No notes at all. A single space stands for "empty" (an empty variable reads as unset).
  await run(
    'update-window-empty',
    [],
    { PT_E2E_UPDATE_OFFER: 'https://example.invalid/none.exe', PT_E2E_UPDATE_NOTES: ' ' },
    TALL,
    async (page) => {
      await openWindow(page)
      const text = await page.locator('[data-update-notes]').first().innerText()
      if (!text.includes(NO_NOTES_TEXT)) problems.push(`update-window-empty at ${s}: the empty line is missing, saw ${JSON.stringify(text)}`)
    }
  )
  return n
}

async function main() {
  const o = parseArgs(process.argv.slice(2))
  if (o.list) {
    console.log(JSON.stringify(SCENES))
    return
  }
  exePath = o.exe ?? DEFAULT_EXE
  if (!existsSync(exePath)) throw new Error(`Prism Terminal is not installed at ${exePath}`)
  const out = resolve(o.out ?? DEFAULT_OUT)
  if (!o.selftest) mkdirSync(out, { recursive: true })
  const scales = o.scales.split(',').map(Number)
  const version = productVersion(exePath)
  if (!version) throw new Error('could not read the product version')
  console.log(`Prism Terminal ${version}`)

  const before = REAL_PROFILES.map(stamp)
  const profile = mkdtempSync(join(tmpdir(), PROFILE_PREFIX))
  if (!resolve(profile).toLowerCase().startsWith(resolve(tmpdir()).toLowerCase())) throw new Error('profile is not under TEMP')
  const problems = []
  let images = 0
  try {
    for (const s of scales) {
      images += await captureScale(s, profile, out, o.selftest, o.only, problems)
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
  if (problems.length) {
    for (const p of problems) console.error('FAIL ' + p)
    process.exit(1)
  }
  if (o.selftest) {
    console.log('ok')
    return
  }
  writeFileSync(join(out, 'VERSION.txt'), version + '\n')
  console.log(`done: ${images} images in ${out}`)
}

main().catch((e) => {
  console.error('FAIL ' + (e?.stack ?? e))
  process.exit(1)
})
