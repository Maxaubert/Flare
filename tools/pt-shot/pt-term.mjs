// Reference capture of the M5 terminal scenes in Prism Terminal, in the same fixed state as
// Flare's `term-*` scenes (crates/flare/src/headless.rs), so the two can sit side by side.
// Throwaway profile, window parked offscreen and unfocusable, the machine's own profile folders
// never touched. The launch block is the one of pt-shot.mjs.
//
//   node pt-term.mjs --out <folder> [--scales 1,1.5,2.25] [--only <text>] [--size 1000x640]
//                    [--exe <PrismTerminal.exe>]
//   node pt-term.mjs --list              (the job list as JSON; compose.ps1 and check.ps1 read it)
//   node pt-term.mjs --selftest-offline  (no app: the scene text and cells against headless.rs)
//   node pt-term.mjs --selftest          (three scenes at scale 1; prints ok)
//
// The images are `<out>/<scene>-<pct>.png` plus `VERSION.txt`. The scene text is what Flare feeds
// its emulator (same rows, written to a file and printed by the shell), the selection is made with
// real mouse events at cell centres computed from the measured cell size, find is opened with
// Ctrl+F, the menus with a right click, the badge with Ctrl+C. Note: Ctrl+C in `term-copied`
// puts the selected text on the machine's clipboard (the app's own copy), as the scene asks.
import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'

const HERE = dirname(fileURLToPath(import.meta.url))
const PRISM_MODULES = 'C:\\Users\\Admin\\Documents\\Claude\\Github\\Prism\\node_modules'
const DEFAULT_EXE = 'C:\\Users\\Admin\\AppData\\Local\\Programs\\PrismTerminal\\PrismTerminal.exe'
const PROFILE_PREFIX = 'pt-term-profile-'
const REAL_PROFILES = ['PrismTerminalStable', 'PrismTerminal'].map((n) => join(process.env.APPDATA ?? '', n))
const PROMPT_SRC = 'PS [^>]*>\\s*$'
const HEADLESS_RS = join(HERE, '..', '..', 'crates', 'flare', 'src', 'headless.rs')

export const SCENES = [
  'term-selection',
  'term-selection-wide',
  'term-find',
  'term-find-empty',
  'term-find-none',
  'term-find-many',
  'term-links',
  'term-menu-selection',
  'term-menu-link',
  'term-menu-path',
  'term-menu-plain',
  'term-copied'
]
export const SCALES3 = [1, 1.5, 2.25]

// The constants of Flare's scene (headless.rs). The offline selftest compares them to the source.
export const PROMPT = 'PS C:\\Users\\Admin\\Documents\\Claude\\Github\\Flare> '
export const RUN_URL = 'https://github.com/Maxaubert/Flare/actions/runs/1'
export const README = 'C:\\Users\\Admin\\Documents\\Claude\\Github\\Flare\\README.md'
export const CWD = 'C:\\Users\\Admin\\Documents\\Claude\\Github\\Flare'

/** The session most scenes show: ten rows, the url on row 5, the path on row 6, the prompt last. */
export function termRows() {
  return [
    `${PROMPT}cargo nextest run`,
    '    Starting 214 tests across 12 binaries',
    'error[E0432]: unresolved import `flare_term::nope`',
    'warning: unused variable `x`',
    '    --> crates\\flare\\src\\main.rs:12:9',
    `    log at ${RUN_URL}`,
    `    source ${README}`,
    '     Summary [   0.412s] 213 tests run: 212 passed, 1 failed',
    'error: test run failed',
    PROMPT
  ]
}

/** The rows a scene feeds its shell (headless.rs `term_text`). `cols` is the grid's width. */
export function termText(name, cols) {
  switch (name) {
    case 'term-selection-wide':
      return ['日本語 emoji 😀 text', 'second row of the block', 'third row of the block', 'fourth row of the block']
    case 'term-find':
    case 'term-find-none':
    case 'term-find-empty':
      return [
        `${PROMPT}cargo nextest run`,
        'error[E0432]: unresolved import',
        'warning: err helper is unused',
        // One match wraps: its "e" ends this row and its "rr" starts the next.
        `${'-'.repeat(Math.max(0, cols - 2))}err tail`,
        'error: test run failed',
        '    Summary: 1 err',
        PROMPT
      ]
    case 'term-find-many':
      return [...termRows(), ...Array.from({ length: 600 }, () => 'err err')]
    case 'term-links':
      return [
        `${PROMPT}cargo nextest run`,
        `    log at ${RUN_URL}`,
        `    source ${README}`,
        // The url starts 20 cells from the right edge, so it wraps over two rows.
        `${' '.repeat(Math.max(0, cols - 20))}${RUN_URL}`,
        PROMPT
      ]
    default:
      return termRows()
  }
}

/**
 * What the shell prints: the scene rows minus the last prompt (the shell prints its own, in the
 * same folder, so the row is the same), each row ended, so the prompt lands on the next row.
 */
export function shellText(name, cols) {
  const rows = termText(name, cols)
  if (rows[rows.length - 1] === PROMPT) rows.pop()
  return rows.join('\r\n') + '\r\n'
}

/**
 * The state each scene brings about, in cells (col, row, right edge of the cell or not) as in
 * headless.rs `stage_term`. `kind` is what the page driver does.
 */
export function sceneSteps(name) {
  const A = (col, row, right = false) => ({ col, row, right })
  switch (name) {
    case 'term-selection':
      return { kind: 'drag', from: A(23, 5), to: A(9, 6, true), alt: false }
    case 'term-selection-wide':
      return { kind: 'drag', from: A(13, 0), to: A(19, 2, true), alt: true }
    case 'term-find':
      return { kind: 'find', query: 'err', step: 'next' }
    case 'term-find-none':
      return { kind: 'find', query: 'zzz', step: '' }
    case 'term-find-many':
      return { kind: 'find', query: 'err', step: 'prev' }
    case 'term-find-empty':
      return { kind: 'find', query: '', step: '' }
    case 'term-links':
      return { kind: 'hover', at: A(20, 1) }
    case 'term-menu-plain':
      return { kind: 'menu', at: A(3, 2) }
    case 'term-menu-selection':
      return { kind: 'menu', select: { from: A(0, 2), to: A(4, 2, true) }, at: A(2, 2) }
    case 'term-menu-link':
      return { kind: 'menu', at: A(20, 5) }
    case 'term-menu-path':
      return { kind: 'menu', at: A(20, 6) }
    case 'term-copied':
      return { kind: 'copied', select: { from: A(0, 2), to: A(4, 2, true) } }
    default:
      throw new Error(`no terminal scene '${name}'`)
  }
}

/** The pointer position of a cell in CSS pixels, from the screen's corner and the measured cell. */
export function cellXY(geo, c) {
  return {
    x: geo.x + (c.col + (c.right ? 0.75 : 0.25)) * geo.cellW,
    y: geo.y + (c.row + 0.5) * geo.cellH
  }
}

export function jobs() {
  const out = []
  for (const s of SCALES3) for (const scene of SCENES) out.push({ name: `${scene}-${Math.round(s * 100)}`, scene, scale: s, pct: Math.round(s * 100) })
  return out
}

function parseArgs(argv) {
  const o = { scales: '', only: '', size: '1000x640', selftest: false, offline: false, list: false }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--selftest') o.selftest = true
    else if (a === '--selftest-offline') o.offline = true
    else if (a === '--list') o.list = true
    else if (a.startsWith('--')) o[a.slice(2)] = argv[++i]
  }
  if (o.selftest) {
    o.scales = '1'
    o.only = 'term-selection-100,term-find-100,term-menu-plain-100'
    o.out ??= mkdtempSync(join(tmpdir(), 'pt-term-self-'))
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
  // A fresh profile opens on the welcome screen: start the first terminal.
  const welcome = page.getByText('New terminal', { exact: true }).first()
  if (!(await page.locator('.xterm').count()) && (await welcome.count())) await welcome.click({ force: true })
  await page.waitForFunction(
    (src) => new RegExp(src).test((document.querySelector('.xterm .xterm-rows')?.textContent ?? '').replace(/\u00a0/g, ' ').trimEnd()),
    PROMPT_SRC,
    { timeout: 60000 }
  )
}

/** The screen's corner (CSS px), the measured cell and the grid's size. */
async function measure(page) {
  return page.evaluate(() => {
    const screen = document.querySelector('.xterm .xterm-screen')
    const rows = document.querySelector('.xterm .xterm-rows')
    const r = screen.getBoundingClientRect()
    // The cell's width from the prompt row itself (plain ASCII): its text's width over its length.
    const row0 = rows.children[0]
    const range = document.createRange()
    range.selectNodeContents(row0)
    const cellW = range.getBoundingClientRect().width / row0.textContent.length
    const cellH = rows.children[0].getBoundingClientRect().height
    return { x: r.x, y: r.y, cellW, cellH, cols: Math.floor(r.width / cellW + 0.01), lines: Math.round(r.height / cellH) }
  })
}

/** Starts the scene's session: a fresh page, the shell in the Flare folder, the text printed. */
async function freshScene(app, page, scene, dir) {
  await page.reload()
  await page.waitForFunction(() => !!window.prism, null, { timeout: 60000 })
  await app.evaluate(park)
  await waitPrompt(page)
  await page.locator('.xterm').first().click({ force: true })
  const geo = await measure(page)
  const file = join(dir, `${scene}.txt`)
  writeFileSync(file, shellText(scene, geo.cols), 'utf8')
  const q = (s) => s.replace(/'/g, "''")
  const cmd = `Set-Location '${q(CWD)}'; [Console]::OutputEncoding=[Text.Encoding]::UTF8; Clear-Host; [Console]::Out.Write([IO.File]::ReadAllText('${q(file)}'))`
  await page.keyboard.type(cmd)
  await page.keyboard.press('Enter')
  // The scene is up when its first row is on screen and the shell's prompt is back below it.
  const first = termText(scene, geo.cols)[0].trim().slice(0, 12)
  await page.waitForFunction(
    ([src, head]) => {
      const t = (document.querySelector('.xterm .xterm-rows')?.textContent ?? '').replace(/\u00a0/g, ' ')
      return t.includes(head) && new RegExp(src).test(t.trimEnd())
    },
    [PROMPT_SRC, first],
    { timeout: 60000 }
  )
  await sleep(500)
  return geo
}

async function stage(page, scene, geo) {
  const step = sceneSteps(scene)
  const at = (c) => cellXY(geo, c)
  const drag = async (from, to, alt) => {
    const a = at(from)
    const b = at(to)
    if (alt) await page.keyboard.down('Alt')
    await page.mouse.move(a.x, a.y)
    await page.mouse.down()
    await page.mouse.move((a.x + b.x) / 2, (a.y + b.y) / 2, { steps: 4 })
    await page.mouse.move(b.x, b.y, { steps: 4 })
    await page.mouse.up()
    if (alt) await page.keyboard.up('Alt')
    await sleep(250)
  }
  let keepPointer = false
  switch (step.kind) {
    case 'drag':
      await drag(step.from, step.to, step.alt)
      break
    case 'find':
      await page.keyboard.press('Control+f')
      await page.waitForSelector('[data-term-find]', { timeout: 10000 })
      if (step.query) {
        await page.keyboard.type(step.query)
        await sleep(800)
        if (step.step === 'next') await page.keyboard.press('Enter')
        if (step.step === 'prev') await page.keyboard.press('Shift+Enter')
        await sleep(600)
      }
      keepPointer = false
      break
    case 'hover': {
      const p = at(step.at)
      await page.mouse.move(p.x - 40, p.y)
      await page.mouse.move(p.x, p.y, { steps: 4 })
      await sleep(500)
      keepPointer = true
      break
    }
    case 'menu': {
      if (step.select) await drag(step.select.from, step.select.to, false)
      const p = at(step.at)
      await page.mouse.move(p.x, p.y, { steps: 2 })
      await page.mouse.click(p.x, p.y, { button: 'right' })
      await sleep(600)
      keepPointer = true
      break
    }
    case 'copied':
      await drag(step.select.from, step.select.to, false)
      await page.keyboard.press('Control+c')
      await page.waitForSelector('[data-copied-badge="shown"]', { timeout: 5000 })
      await sleep(150)
      keepPointer = true
      break
    default:
      break
  }
  // The pointer rests in a corner, so no hover fill is in the picture (unless the scene is the hover).
  if (!keepPointer) await page.mouse.move(2, 638)
}

/** The checks the offline selftest runs: the scene rows against the Flare source. */
export function offlineProblems(source) {
  const bad = []
  const rs = (name) => new RegExp(`const ${name}: &str = r?"([^"]*)";`).exec(source)?.[1]
  const want = { PROMPT, RUN_URL, README }
  for (const [k, v] of Object.entries(want)) {
    const got = rs(k)
    if (got === undefined) bad.push(`headless.rs has no const ${k}`)
    else if (got !== v) bad.push(`${k} differs: "${got}" in headless.rs, "${v}" here`)
  }
  for (const scene of SCENES) {
    if (!source.includes(`"${scene}"`)) bad.push(`headless.rs has no scene ${scene}`)
    try {
      sceneSteps(scene)
    } catch (e) {
      bad.push(String(e.message))
    }
  }
  const rows = termRows()
  if (rows.length !== 10 || !rows[5].includes(RUN_URL) || !rows[6].includes(README)) bad.push('term_rows: the url must be on row 5 and the path on row 6')
  const links = termText('term-links', 100)
  if (links[3] !== ' '.repeat(80) + RUN_URL) bad.push('term-links: the url must start 20 cells from the right edge')
  const find = termText('term-find', 100)
  if (!find[3].startsWith('-'.repeat(98) + 'err')) bad.push('term-find: a match must wrap over rows 3 and 4')
  if (termText('term-find-many', 100).length !== 610) bad.push('term-find-many: 10 rows and 600 lines')
  if (shellText('term-find', 100).endsWith(PROMPT) || !shellText('term-find', 100).endsWith('\r\n')) bad.push('shellText must drop the last prompt and end the row')
  const c = cellXY({ x: 10, y: 20, cellW: 8, cellH: 16 }, { col: 2, row: 1, right: true })
  if (c.x !== 10 + 2.75 * 8 || c.y !== 20 + 1.5 * 16) bad.push('cellXY: wrong centre')
  if (jobs().length !== SCENES.length * SCALES3.length) bad.push('jobs: wrong count')
  return bad
}

async function main() {
  const o = parseArgs(process.argv.slice(2))
  if (o.list) {
    console.log(JSON.stringify(jobs()))
    return
  }
  if (o.offline) {
    const bad = offlineProblems(readFileSync(HEADLESS_RS, 'utf8'))
    if (bad.length) {
      for (const p of bad) console.error('FAIL ' + p)
      process.exit(1)
    }
    console.log('ok')
    return
  }
  const require = createRequire(join(PRISM_MODULES, 'noop.js'))
  const { _electron: electron } = require('playwright-core')
  if (!o.out) throw new Error('--out <folder> is required')
  const exe = o.exe ?? DEFAULT_EXE
  if (!existsSync(exe)) throw new Error(`Prism Terminal is not installed at ${exe}`)
  const [W, H] = o.size.split('x').map(Number)
  const wanted = o.scales ? o.scales.split(',').map(Number) : null
  const only = o.only ? o.only.split(',').filter(Boolean) : []
  const list = jobs().filter((j) => (!wanted || wanted.includes(j.scale)) && (!only.length || only.some((t) => j.name.includes(t))))
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
            const geo = await freshScene(app, page, job.scene, profile)
            await stage(page, job.scene, geo)
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
