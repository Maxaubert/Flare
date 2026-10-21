// Reference capture: screenshots the INSTALLED Prism Terminal wearing each theme, so Flare's
// theme matrix (Task 30) can sit beside it. Throwaway profile, window parked offscreen and
// unfocusable, the machine's own profile folders never touched.
//
//   node pt-shot.mjs --out <folder> [--scales 1,1.5,2.25] [--themes all|id,id] [--size 1000x640]
//                    [--sample <file>] [--exe <PrismTerminal.exe>]
//   node pt-shot.mjs --selftest
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
const PROFILE_PREFIX = 'pt-shot-profile-'
const REAL_PROFILES = ['PrismTerminalStable', 'PrismTerminal'].map((n) => join(process.env.APPDATA ?? '', n))
const PROMPT_SRC = 'PS [^>]*>\\s*$'
const ALL_THEMES = join(HERE, '..', 'oracle', 'out', 'wall_order.json')

function parseArgs(argv) {
  const o = { scales: '1,1.5,2.25', themes: 'all', size: '1000x640', selftest: false }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--selftest') o.selftest = true
    else if (a.startsWith('--')) o[a.slice(2)] = argv[++i]
  }
  if (o.selftest) {
    o.themes = 'pt-default'
    o.scales = '1'
    o.out ??= mkdtempSync(join(tmpdir(), 'pt-shot-self-'))
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

async function waitPrompt(page) {
  if (process.env.PT_SHOT_DEBUG) {
    setTimeout(async () => {
      const t = await page.evaluate(() => document.querySelector('.xterm .xterm-rows')?.textContent ?? document.body.innerText.slice(0, 300))
      console.log('ROWS', JSON.stringify(t))
    }, 15000)
  }
  // A fresh profile opens on the welcome screen: start the first terminal.
  const welcome = page.getByText('New terminal', { exact: true }).first()
  if (!(await page.locator('.xterm').count()) && (await welcome.count())) await welcome.click({ force: true })
  await page.waitForFunction(
    (src) => new RegExp(src).test((document.querySelector('.xterm .xterm-rows')?.textContent ?? '').trimEnd()),
    PROMPT_SRC,
    { timeout: 60000 }
  )
}

function themeIds(spec) {
  if (spec !== 'all') return spec.split(',').filter(Boolean)
  if (!existsSync(ALL_THEMES)) throw new Error(`--themes all needs ${ALL_THEMES} (Task 2's oracle output)`)
  const j = JSON.parse(readFileSync(ALL_THEMES, 'utf8'))
  const ids = (Array.isArray(j) ? j : Object.keys(j)).map((x) => (typeof x === 'string' ? x : x.id))
  if (!ids.length) throw new Error('no theme ids in the oracle output')
  return ids
}

async function main() {
  const o = parseArgs(process.argv.slice(2))
  if (!o.out) throw new Error('--out <folder> is required')
  const exe = o.exe ?? DEFAULT_EXE
  if (!existsSync(exe)) throw new Error(`Prism Terminal is not installed at ${exe}`)
  const sample = resolve(o.sample ?? join(HERE, '..', '..', 'docs', 'theme-sample.txt'))
  if (!existsSync(sample)) throw new Error(`the sample file is missing: ${sample}`)
  const [W, H] = o.size.split('x').map(Number)
  const scales = o.scales.split(',').map(Number)
  const themes = themeIds(o.themes)
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
        // The window adds invisible frame to a content size (1016x648 for 1000x640): ask,
        // measure the page, and correct until the page itself is W x H.
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
        for (const id of themes) {
          await page.evaluate((t) => localStorage.setItem('prism.term.theme', t), id)
          await page.reload()
          await page.waitForFunction(() => !!window.prism, null, { timeout: 60000 })
          await app.evaluate(park)
          await waitPrompt(page)
          await page.locator('.xterm').first().click({ force: true })
          const cmd = `[Console]::Out.Write([IO.File]::ReadAllText('${sample.replace(/'/g, "''")}'))`
          await page.keyboard.type(cmd)
          await page.keyboard.press('Enter')
          await page.waitForFunction(
            () => (document.querySelector('.xterm .xterm-rows')?.textContent ?? '').includes('Flare theme sample'),
            null,
            { timeout: 30000 }
          )
          await sleep(300)
          const pct = Math.round(s * 100)
          const file = join(out, `${id}-${pct}.png`)
          await page.screenshot({ path: file, clip: { x: 0, y: 0, width: W, height: H } })
          shots.push({ file, s })
          console.log(`  ${id} @ ${pct}%`)
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

  const problems = []
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

main().catch((e) => {
  console.error('FAIL ' + (e?.stack ?? e))
  process.exit(1)
})
