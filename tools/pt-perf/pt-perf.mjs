// Measures the INSTALLED Prism Terminal the way Flare is measured (plan M10, Decisions D5 and D7),
// so docs/perf/bar.json can hold "flare <= pt / 3" style rules against a baseline from this PC.
// The launch block is copied from tools/pt-shot/pt-shot.mjs: Playwright's Electron driver from the
// Prism repo's node_modules, a throwaway --user-data-dir under %TEMP%, --e2e (the window is parked
// offscreen and unfocusable), reaped by profile path only. It never touches %APPDATA%\PrismTerminal*
// and never takes the foreground.
//
//   node pt-perf.mjs --out <json> [--only start,memory,flood,latency,scroll,idle] [--runs 7]
//   node pt-perf.mjs --selftest          tiny sizes, 1 run each, prints the same JSON shape, exit 1 on a missing field
//   node pt-perf.mjs --selftest-offline  no app is started: checks the shape checker and the statistics only
//
// Every number and its unit is listed in README.md.
import { execFileSync } from 'node:child_process'
import { closeSync, existsSync, mkdtempSync, openSync, rmSync, statSync, writeFileSync, writeSync } from 'node:fs'
import { mkdirSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'

const HERE = dirname(fileURLToPath(import.meta.url))
const PRISM_MODULES = 'C:\\Users\\Admin\\Documents\\Claude\\Github\\Prism\\node_modules'
const DEFAULT_EXE = 'C:\\Users\\Admin\\AppData\\Local\\Programs\\PrismTerminal\\PrismTerminal.exe'
const PROFILE_PREFIX = 'pt-perf-profile-'
const REAL_PROFILES = ['PrismTerminalStable', 'PrismTerminal'].map((n) => join(process.env.APPDATA ?? '', n))
const SECTIONS = ['start', 'memory', 'flood', 'latency', 'scroll', 'idle']
// Processes that are the shell and what it starts, not the terminal (D5).
const SHELL_NAMES = ['cmd', 'pwsh', 'powershell', 'openconsole', 'conhost', 'node']
const PROMPT = { cmd: '>\\s*$', pwsh: 'PS [^>]*>\\s*$' }
const SHELL_ID = { cmd: 'cmd', pwsh: 'pwsh' }

// ---------------------------------------------------------------- pure helpers (tested offline)

export function median(xs) {
  if (!xs.length) return 0
  const s = [...xs].sort((a, b) => a - b)
  const m = s.length >> 1
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2
}

/** Nearest-rank percentile, p in 0..100. */
export function percentile(xs, p) {
  if (!xs.length) return 0
  const s = [...xs].sort((a, b) => a - b)
  const rank = Math.min(s.length, Math.max(1, Math.ceil((p / 100) * s.length)))
  return s[rank - 1]
}

const r2 = (n) => Math.round(n * 100) / 100

export function frameStats(times) {
  const gaps = []
  for (let i = 1; i < times.length; i++) gaps.push(times[i] - times[i - 1])
  return {
    count: times.length,
    p95_ms: r2(percentile(gaps, 95)),
    max_ms: r2(gaps.length ? Math.max(...gaps) : 0),
    over_33: gaps.filter((g) => g > 33).length
  }
}

/** The shape every run prints. `only` limits which sections must be present. */
export function shapeTemplate(only = SECTIONS) {
  const startKeys = { runs: [0], median: 0, prompt_median: 0, echo_median: 0, outside_median: 0, late_hook: 0 }
  const t = { pt_version: '', taken: '', machine: { cpu: '', gpu: '', primary_scale: 0, windows: '' } }
  if (only.includes('start')) t.start_ms = { cmd: startKeys, pwsh: startKeys }
  if (only.includes('memory')) t.idle_private_ws_mb = { cmd: { processes: [{ name: '', pid: 0, mb: 0 }], total: 0 } }
  if (only.includes('flood')) t.flood = { mb: 0, seconds: 0, mb_per_s: 0, frames: { count: 0, p95_ms: 0, max_ms: 0, over_33: 0 } }
  if (only.includes('latency')) t.key_latency_ms = { n: 0, median: 0, p95: 0, max: 0 }
  if (only.includes('scroll')) t.scroll = { seconds: 0, fps: 0, max_frame_ms: 0 }
  if (only.includes('idle')) t.idle_cpu_pct_of_core = { cmd: 0 }
  return t
}

/** Returns the list of problems (empty when the value has every key of the template with the right type). */
export function checkShape(value, tmpl, path = '$') {
  const bad = []
  if (Array.isArray(tmpl)) {
    if (!Array.isArray(value)) return [`${path}: not an array`]
    const el = tmpl[0]
    value.forEach((v, i) => bad.push(...checkShape(v, el, `${path}[${i}]`)))
    return bad
  }
  if (tmpl !== null && typeof tmpl === 'object') {
    if (value === null || typeof value !== 'object' || Array.isArray(value)) return [`${path}: not an object`]
    for (const k of Object.keys(tmpl)) {
      if (!(k in value)) bad.push(`${path}.${k}: missing`)
      else bad.push(...checkShape(value[k], tmpl[k], `${path}.${k}`))
    }
    return bad
  }
  if (typeof tmpl === 'number') {
    if (typeof value !== 'number' || !Number.isFinite(value)) bad.push(`${path}: not a finite number`)
  } else if (typeof tmpl === 'string') {
    if (typeof value !== 'string') bad.push(`${path}: not a string`)
  }
  return bad
}

export function startSummary(runs) {
  return {
    runs: runs.map((r) => r2(r.prompt + r.echo)),
    median: r2(median(runs.map((r) => r.prompt + r.echo))),
    prompt_median: r2(median(runs.map((r) => r.prompt))),
    echo_median: r2(median(runs.map((r) => r.echo))),
    outside_median: r2(median(runs.map((r) => r.outside))),
    late_hook: runs.filter((r) => r.late).length
  }
}

// ---------------------------------------------------------------- the machine

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

function ps(script) {
  return execFileSync('powershell.exe', ['-NoProfile', '-Command', script], { encoding: 'utf8', windowsHide: true, maxBuffer: 1 << 26 }).trim()
}

function stamp(p) {
  try {
    return statSync(p).mtimeMs
  } catch {
    return null
  }
}

function productVersion(exe) {
  return ps(`(Get-Item -LiteralPath '${exe.replace(/'/g, "''")}').VersionInfo.ProductVersion`)
}

function machineInfo() {
  const dpi = `Add-Type -Namespace W -Name U -MemberDefinition '[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern uint GetDpiForSystem();'; [W.U]::GetDpiForSystem()`
  const j = ps(
    `$o=[ordered]@{ cpu=(Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name); ` +
      `gpu=((Get-CimInstance Win32_VideoController | ForEach-Object Name) -join ' + '); ` +
      `dpi=[int]$(${dpi}); windows=$((Get-CimInstance Win32_OperatingSystem).Caption + ' ' + (Get-CimInstance Win32_OperatingSystem).Version) }; $o | ConvertTo-Json -Compress`
  )
  const m = JSON.parse(j)
  return { cpu: String(m.cpu ?? '').trim(), gpu: String(m.gpu ?? ''), primary_scale: (m.dpi || 96) / 96, windows: String(m.windows ?? '') }
}

/** Kill only processes whose command line names OUR throwaway profile. */
function reap(profile) {
  try {
    const q = "Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '*" + profile.replace(/'/g, "''") + "*' } | ForEach-Object { $_.ProcessId }"
    for (const pid of ps(q).split(/\s+/).filter(Boolean)) {
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

/** The app's own process tree (main plus every descendant) minus the shell and what it starts. */
function treeRows(rootPid) {
  const script =
    `$all = Get-CimInstance Win32_Process; $keep = @{}; $keep[${rootPid}] = $true; ` +
    `do { $n = 0; foreach ($p in $all) { if ($keep.ContainsKey([int]$p.ParentProcessId) -and -not $keep.ContainsKey([int]$p.ProcessId)) { $keep[[int]$p.ProcessId] = $true; $n++ } } } while ($n -gt 0); ` +
    `$perf = @{}; Get-CimInstance Win32_PerfFormattedData_PerfProc_Process | ForEach-Object { $perf[[int]$_.IDProcess] = $_.WorkingSetPrivate }; ` +
    `$all | Where-Object { $keep.ContainsKey([int]$_.ProcessId) } | ForEach-Object { [pscustomobject]@{ name = ($_.Name -replace '\\.exe$',''); pid = [int]$_.ProcessId; priv = [double]$perf[[int]$_.ProcessId]; cpu = [double]($_.KernelModeTime + $_.UserModeTime) } } | ConvertTo-Json -Compress`
  const out = ps(script)
  if (!out) return []
  const rows = JSON.parse(out)
  return (Array.isArray(rows) ? rows : [rows]).filter((r) => !SHELL_NAMES.includes(String(r.name).toLowerCase()))
}

// ---------------------------------------------------------------- the page side

/** Installed before the page runs. Everything is read off the DOM terminal rows, as the e2e does. */
const PAGE_HOOK = ([promptSrc, shellId]) => {
  const w = window
  try {
    localStorage.setItem('prism.term.shell', shellId)
  } catch {
    /* no storage yet */
  }
  const P = (w.__perf = { promptAt: null, late: false, checks: 0 })
  const rowsText = () => (document.querySelector('.xterm .xterm-rows')?.textContent ?? '').replace(/\u00a0/g, ' ')
  P.rowsText = rowsText
  const re = new RegExp(promptSrc)
  const tick = () => {
    // A fresh profile opens on the welcome screen: start the first terminal as soon as it shows.
    if (!document.querySelector('.xterm')) {
      // 0.58.0: the welcome button holds the label and a "Ctrl+T" hint, so match the text that starts with the label.
      const b = [...document.querySelectorAll('button, [role=button]')].find((e) => (e.textContent ?? '').trim().startsWith('New terminal'))
      b?.click()
    }
    if (P.promptAt === null && re.test(rowsText().trimEnd())) {
      P.promptAt = Date.now()
      P.late = P.checks === 0
    }
    P.checks += 1
    if (P.promptAt === null) requestAnimationFrame(tick)
  }
  requestAnimationFrame(tick)
}

/** Records key-to-DOM-render latency: keydown (capture) to the mutation that shows the echoed run of "a". */
const LATENCY_HOOK = () => {
  const P = window.__perf
  P.keyStart = []
  P.keyEnd = []
  addEventListener('keydown', () => P.keyStart.push(performance.now()), true)
  const rows = document.querySelector('.xterm .xterm-rows')
  new MutationObserver(() => {
    const run = (P.rowsText().trimEnd().match(/a+$/) ?? [''])[0].length
    while (P.keyEnd.length < Math.min(run, P.keyStart.length)) P.keyEnd.push(performance.now())
  }).observe(rows, { subtree: true, childList: true, characterData: true })
}

/** Frame times from now until the buffer's last non-empty row holds the marker. */
const FRAMES_HOOK = (marker) => {
  const P = window.__perf
  P.frames = []
  P.done = false
  const tick = (t) => {
    P.frames.push(t)
    const rows = P.rowsText()
    if (rows.includes(marker)) {
      P.done = true
      P.doneAt = t
      return
    }
    requestAnimationFrame(tick)
  }
  requestAnimationFrame(tick)
}

// ---------------------------------------------------------------- one launched app

async function withApp(o, shell, fn) {
  const require = createRequire(join(PRISM_MODULES, 'noop.js'))
  const { _electron: electron } = require('playwright-core')
  const udd = mkdtempSync(join(o.profileRoot, 'p-'))
  const t0 = performance.now()
  const app = await electron.launch({ executablePath: o.exe, args: [`--user-data-dir=${udd}`, '--e2e'], env: { ...process.env } })
  try {
    const ctx = app.context()
    await ctx.addInitScript(PAGE_HOOK, [PROMPT[shell], SHELL_ID[shell]]).catch(() => {})
    const page = await app.firstWindow()
    await app.evaluate(({ BrowserWindow }) => {
      for (const w of BrowserWindow.getAllWindows()) {
        w.setOpacity(0)
        w.setPosition(-4000, -4000)
      }
    })
    await page.waitForFunction(() => !!window.prism, null, { timeout: 60000 })
    // The init script may have missed this first load (Playwright attaches after the window exists):
    // install it by hand then. The run is kept and counted in late_hook by the page's own check.
    const hooked = await page.evaluate(() => !!window.__perf)
    if (!hooked) await page.evaluate(`(${PAGE_HOOK.toString()})(${JSON.stringify([PROMPT[shell], SHELL_ID[shell]])})`)
    return await fn({ app, page, udd, t0, electron })
  } finally {
    await app.close().catch(() => {})
    reap(udd)
    rmSync(udd, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 })
  }
}

async function waitPrompt(page, timeout = 60000) {
  try {
    await page.waitForFunction(() => window.__perf?.promptAt != null, null, { timeout, polling: 20 })
  } catch (e) {
    const seen = await page
      .evaluate(() => `xterm=${!!document.querySelector('.xterm')} rows=${JSON.stringify((document.querySelector('.xterm .xterm-rows')?.textContent ?? '').slice(0, 200))} body=${JSON.stringify(document.body.innerText.slice(0, 300))}`)
      .catch((x) => `unreadable: ${x}`)
    throw new Error(`no prompt in ${timeout} ms: ${seen}`, { cause: e })
  }
}

async function focusTerm(page) {
  await page.locator('.xterm').first().click({ force: true })
}

async function typeLine(page, text) {
  await page.keyboard.type(text)
  await page.keyboard.press('Enter')
}

// ---------------------------------------------------------------- the measurements

async function measureStart(o, shell) {
  const one = () =>
    withApp(o, shell, async ({ app, page, t0 }) => {
      const zero = await app.evaluate(() => process.getCreationTime())
      await waitPrompt(page)
      const { promptAt, late } = await page.evaluate(() => ({ promptAt: window.__perf.promptAt, late: window.__perf.late }))
      await page.evaluate(() => {
        const P = window.__perf
        P.echoStart = null
        P.echoEnd = null
        addEventListener('keydown', () => (P.echoStart ??= performance.now()), true)
        const rows = document.querySelector('.xterm .xterm-rows')
        new MutationObserver(() => {
          if (P.echoStart !== null && P.echoEnd === null && P.rowsText().trimEnd().endsWith('x')) P.echoEnd = performance.now()
        }).observe(rows, { subtree: true, childList: true, characterData: true })
      })
      await focusTerm(page)
      await page.keyboard.press('x')
      await page.waitForFunction(() => window.__perf.echoEnd !== null, null, { timeout: 10000, polling: 10 })
      const echo = await page.evaluate(() => window.__perf.echoEnd - window.__perf.echoStart)
      return { prompt: promptAt - zero, echo, outside: performance.now() - t0, late }
    })
  await one() // one discarded launch
  const runs = []
  for (let i = 0; i < o.runs; i++) runs.push(await one())
  return startSummary(runs)
}

async function measureMemory(o, shell) {
  const sessions = []
  for (let i = 0; i < o.memSessions; i++) {
    sessions.push(
      await withApp(o, shell, async ({ app, page }) => {
        await waitPrompt(page)
        await sleep(o.idleWaitMs)
        const rows = treeRows(app.process().pid)
        const processes = rows.map((r) => ({ name: r.name, pid: r.pid, mb: r2((r.priv || 0) / 1048576) }))
        return { processes, total: r2(processes.reduce((a, p) => a + p.mb, 0)) }
      })
    )
  }
  const mid = sessions.slice().sort((a, b) => a.total - b.total)[sessions.length >> 1]
  return { processes: mid.processes, total: mid.total }
}

/** 100-byte lines (98 text bytes plus CRLF); the last line is FLOOD-END. Returns the file's byte size. */
function writeFlood(file, lines) {
  const fd = openSync(file, 'w')
  try {
    let buf = ''
    for (let i = 1; i < lines; i++) {
      buf += `LINE ${String(i).padStart(9, '0')} `.padEnd(98, 'x') + '\r\n'
      if (buf.length > 1 << 20) {
        writeSync(fd, buf)
        buf = ''
      }
    }
    buf += 'FLOOD-END\r\n'
    writeSync(fd, buf)
  } finally {
    closeSync(fd)
  }
  return statSync(file).size
}

async function floodOnce(o, page, file) {
  await focusTerm(page)
  await page.evaluate(() => {
    window.__perf.floodStart = performance.now()
  })
  await page.evaluate(FRAMES_HOOK, 'FLOOD-END')
  const t = Date.now()
  await typeLine(page, `type "${file}"`)
  await page.waitForFunction(() => window.__perf.done === true, null, { timeout: o.floodTimeoutMs, polling: 50 })
  const seconds = (Date.now() - t) / 1000
  const frames = await page.evaluate(() => window.__perf.frames)
  const from = frames.length ? frames[0] + 500 : 0
  return { seconds, frames: frames.filter((f) => f >= from) }
}

async function measureFlood(o, shell) {
  return withApp(o, shell, async ({ page, udd }) => {
    await waitPrompt(page)
    const file = join(udd, 'flood.txt')
    const lines = Math.round((o.floodMb * 1048576) / 100)
    const bytes = writeFlood(file, lines)
    const { seconds, frames } = await floodOnce(o, page, file)
    const mb = bytes / 1048576
    return { mb: r2(mb), seconds: r2(seconds), mb_per_s: r2(mb / seconds), frames: frameStats(frames) }
  })
}

async function measureLatency(o, shell) {
  return withApp(o, shell, async ({ page }) => {
    await waitPrompt(page)
    await focusTerm(page)
    await page.evaluate(LATENCY_HOOK)
    for (let i = 0; i < o.keys; i++) {
      await page.keyboard.press('a')
      await sleep(20)
    }
    await page.waitForFunction((n) => window.__perf.keyEnd.length >= n, o.keys, { timeout: 15000, polling: 20 })
    const { s, e } = await page.evaluate(() => ({ s: window.__perf.keyStart, e: window.__perf.keyEnd }))
    const lat = e.slice(0, o.keys).map((end, i) => end - s[i])
    return { n: lat.length, median: r2(median(lat)), p95: r2(percentile(lat, 95)), max: r2(Math.max(...lat)) }
  })
}

async function measureScroll(o, shell) {
  return withApp(o, shell, async ({ page, udd }) => {
    await waitPrompt(page)
    const file = join(udd, 'scroll.txt')
    writeFlood(file, o.scrollLines)
    await floodOnce(o, page, file)
    await page.evaluate(() => {
      const P = window.__perf
      P.sframes = []
      P.srun = true
      const tick = (t) => {
        P.sframes.push(t)
        if (P.srun) requestAnimationFrame(tick)
      }
      requestAnimationFrame(tick)
    })
    const box = await page.locator('.xterm').first().boundingBox()
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2)
    const t0 = Date.now()
    for (let i = 0; i < o.wheels; i++) {
      await page.mouse.wheel(0, -120)
      const due = t0 + ((i + 1) * o.scrollSeconds * 1000) / o.wheels
      const wait = due - Date.now()
      if (wait > 1) await sleep(wait)
    }
    const secs = (Date.now() - t0) / 1000
    const fr = await page.evaluate(() => {
      window.__perf.srun = false
      return window.__perf.sframes
    })
    const st = frameStats(fr)
    return { seconds: r2(secs), fps: r2(fr.length / secs), max_frame_ms: st.max_ms }
  })
}

async function measureIdle(o, shell) {
  return withApp(o, shell, async ({ app, page }) => {
    await waitPrompt(page)
    await sleep(o.idleWaitMs)
    const pid = app.process().pid
    const a = treeRows(pid)
    const ta = Date.now()
    await sleep(o.cpuWindowMs)
    const b = treeRows(pid)
    const dt = (Date.now() - ta) / 1000
    const used = new Map(a.map((r) => [r.pid, r.cpu]))
    let units = 0
    for (const r of b) units += r.cpu - (used.get(r.pid) ?? r.cpu) // 100 ns units; a process that appeared counts from zero change
    return r2(((units / 1e7) / dt) * 100)
  })
}

// ---------------------------------------------------------------- main

function parseArgs(argv) {
  const o = { runs: 7, selftest: false, offline: false }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--selftest') o.selftest = true
    else if (a === '--selftest-offline') o.offline = true
    else if (a.startsWith('--')) o[a.slice(2)] = argv[++i]
  }
  o.runs = Number(o.runs)
  o.only = o.only ? String(o.only).split(',').filter(Boolean) : SECTIONS
  return o
}

function sizes(self) {
  return self
    ? { floodMb: 1, scrollLines: 3000, keys: 20, wheels: 50, scrollSeconds: 1, memSessions: 1, idleWaitMs: 1500, cpuWindowMs: 2000, floodTimeoutMs: 60000, runs: 1 }
    : { floodMb: 100, scrollLines: 100000, keys: 200, wheels: 1000, scrollSeconds: 5, memSessions: 3, idleWaitMs: 10000, cpuWindowMs: 10000, floodTimeoutMs: 300000 }
}

async function run(o) {
  const exe = o.exe ?? DEFAULT_EXE
  if (!existsSync(exe)) throw new Error(`Prism Terminal is not installed at ${exe}`)
  const profileRoot = mkdtempSync(join(tmpdir(), PROFILE_PREFIX))
  if (!resolve(profileRoot).toLowerCase().startsWith(resolve(tmpdir()).toLowerCase())) throw new Error('profile is not under TEMP')
  const cfg = { ...o, ...sizes(o.selftest), exe, profileRoot }
  if (o.selftest) cfg.runs = 1
  const before = REAL_PROFILES.map(stamp)
  const out = { pt_version: productVersion(exe), taken: new Date().toISOString(), machine: machineInfo() }
  try {
    const want = (s) => o.only.includes(s)
    if (want('start')) {
      out.start_ms = {}
      // cmd is the gated shell; pwsh is reported beside it, without a bar (D5).
      for (const sh of ['cmd', 'pwsh']) out.start_ms[sh] = await measureStart(cfg, sh)
    }
    if (want('memory')) out.idle_private_ws_mb = { cmd: await measureMemory(cfg, 'cmd') }
    if (want('flood')) out.flood = await measureFlood(cfg, 'cmd')
    if (want('latency')) out.key_latency_ms = await measureLatency(cfg, 'cmd')
    if (want('scroll')) out.scroll = await measureScroll(cfg, 'cmd')
    if (want('idle')) out.idle_cpu_pct_of_core = { cmd: await measureIdle(cfg, 'cmd') }
  } finally {
    reap(profileRoot)
    rmSync(profileRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 })
  }
  const problems = checkShape(out, shapeTemplate(o.only))
  if (existsSync(profileRoot)) problems.push(`profile ${profileRoot} was not removed`)
  REAL_PROFILES.forEach((p, i) => {
    if (before[i] !== stamp(p)) problems.push(`real profile touched: ${p}`)
  })
  return { out, problems }
}

function selftestOffline() {
  const bad = []
  const must = (c, m) => c || bad.push(m)
  must(median([3, 1, 2]) === 2 && median([1, 2, 3, 4]) === 2.5 && median([]) === 0, 'median')
  must(percentile([1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 95) === 10 && percentile([5], 95) === 5, 'percentile')
  const fs = frameStats([0, 16, 32, 100, 116])
  must(fs.count === 5 && fs.max_ms === 68 && fs.over_33 === 1, 'frameStats ' + JSON.stringify(fs))
  const sum = startSummary([
    { prompt: 100, echo: 10, outside: 400, late: true },
    { prompt: 120, echo: 12, outside: 420, late: false },
    { prompt: 110, echo: 11, outside: 410, late: false }
  ])
  must(sum.median === 121 && sum.late_hook === 1 && sum.runs.length === 3, 'startSummary ' + JSON.stringify(sum))
  // A full result passes; every missing or mistyped key fails.
  const full = {
    pt_version: '0.58.0.0',
    taken: new Date().toISOString(),
    machine: { cpu: 'x', gpu: 'y', primary_scale: 2.25, windows: 'z' },
    start_ms: { cmd: sum, pwsh: sum },
    idle_private_ws_mb: { cmd: { processes: [{ name: 'a', pid: 1, mb: 2 }], total: 2 } },
    flood: { mb: 100, seconds: 1, mb_per_s: 100, frames: fs },
    key_latency_ms: { n: 200, median: 1, p95: 2, max: 3 },
    scroll: { seconds: 5, fps: 60, max_frame_ms: 20 },
    idle_cpu_pct_of_core: { cmd: 0.1 }
  }
  must(checkShape(full, shapeTemplate()).length === 0, 'a complete result must pass: ' + checkShape(full, shapeTemplate()))
  must(checkShape(structuredClone(full), shapeTemplate(['flood'])).length === 0, '--only flood checks flood alone')
  const noKey = structuredClone(full)
  delete noKey.flood.frames.over_33
  must(checkShape(noKey, shapeTemplate()).some((p) => p.includes('over_33')), 'a missing key must fail')
  const nan = structuredClone(full)
  nan.scroll.fps = NaN
  must(checkShape(nan, shapeTemplate()).length === 1, 'a NaN must fail')
  const str = structuredClone(full)
  str.key_latency_ms.p95 = '3'
  must(checkShape(str, shapeTemplate()).length === 1, 'a string where a number goes must fail')
  // The flood writer makes 100-byte lines and a last FLOOD-END line.
  const dir = mkdtempSync(join(tmpdir(), 'pt-perf-self-'))
  try {
    const f = join(dir, 'f.txt')
    const bytes = writeFlood(f, 1000)
    must(bytes === 999 * 100 + 11, `flood size ${bytes}`)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
  return bad
}

async function main() {
  const o = parseArgs(process.argv.slice(2))
  if (o.offline) {
    const bad = selftestOffline()
    for (const b of bad) console.error('FAIL ' + b)
    if (bad.length) process.exit(1)
    console.log('ok (offline)')
    return
  }
  if (!o.selftest && !o.out) throw new Error('--out <json> is required')
  const { out, problems } = await run(o)
  const text = JSON.stringify(out, null, 2)
  if (o.out) {
    mkdirSync(dirname(resolve(o.out)), { recursive: true })
    writeFileSync(resolve(o.out), text + '\n')
  }
  console.log(text)
  if (problems.length) {
    for (const p of problems) console.error('FAIL ' + p)
    process.exit(1)
  }
  if (o.selftest) console.log('ok')
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((e) => {
    console.error('FAIL ' + (e?.stack ?? e))
    process.exit(1)
  })
}
