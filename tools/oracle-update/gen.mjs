// THE ORACLE (Task 2 of the M9 plan). Runs Prism Terminal's own TypeScript
// (read from PT_REF, read-only) on a seeded corpus and writes the golden
// fixtures the Flare tests compare against. Run: node tools/oracle-update/gen.mjs
// Idempotent: the seeds and sort orders are fixed, so a second run leaves git clean.

import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { stripTypeScriptTypes } from 'node:module'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { EDGE, TITLES, bodies, mulberry32 } from './corpus.mjs'
import { readPtNames } from './names.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
const REPO = resolve(HERE, '../..')
const PT = resolve(
  process.env.PT_REF ??
    'C:/Users/Admin/AppData/Local/Temp/claude/C--Users-Admin-Documents-Claude-Github-Prism/43cf2307-096a-420d-ae97-1aa724ab8df6/scratchpad/pt-ref',
)
const load = (rel) => import(pathToFileURL(join(PT, rel)).href)

const { parseReleaseNotes, MAX_ENTRIES, MAX_ENTRY_CHARS } = await load('core/shared/releaseNotes.ts')
const { groupReleaseNotes, classifyEntry } = await load('core/shared/releaseGroups.ts')
const { updateFlow, NO_UPDATE } = await load('core/renderer/lib/updateFlow.ts')
const { previewVersion, wantsPreview, SAMPLE_NOTES, runPreviewInstall, previewUpdate, PREVIEW_MS } = await load('core/main/updatePreview.ts')
const { capString, summariseValue, summariseArgs, errorFields, cleanFields } = await load('core/main/diagSummary.ts')
const { createDiagGate } = await load('core/shared/diagGate.ts')

const FIX = {
  version: join(REPO, 'crates/flare-update/tests/fixtures'),
  diag: join(REPO, 'crates/flare-diag/tests/fixtures'),
  win: join(REPO, 'crates/flare-win/tests/fixtures'),
}

const write = (path, obj) => {
  mkdirSync(dirname(path), { recursive: true })
  writeFileSync(path, JSON.stringify(obj, null, 2) + '\n')
}

// ---------------------------------------------------------------- shared helpers

const LONE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/
const LONE_G = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g
const hasLone = (v) =>
  typeof v === 'string' ? LONE.test(v) : Array.isArray(v) ? v.some(hasLone) : v && typeof v === 'object' ? Object.values(v).some(hasLone) : false
const stripLone = (v) =>
  typeof v === 'string'
    ? v.replace(LONE_G, '')
    : Array.isArray(v)
      ? v.map(stripLone)
      : v && typeof v === 'object'
        ? Object.fromEntries(Object.entries(v).map(([k, x]) => [k, stripLone(x)]))
        : v

/** A JSON spec turned into the value it stands for. Special keys:
 *  {undefined}, {number:"NaN"|"Infinity"}, {bigint}, {fn}, {symbol}, {$bytes},
 *  {$arraybuffer}, {$error:{message,stack?}}, {repeat:[s,n]}, {concat:[...]}. */
function decodeSpec(spec) {
  if (spec === null || typeof spec !== 'object') return spec
  if (Array.isArray(spec)) return spec.map(decodeSpec)
  if ('undefined' in spec) return undefined
  if ('repeat' in spec) return spec.repeat[0].repeat(spec.repeat[1])
  if ('concat' in spec) return spec.concat.map((x) => decodeSpec(x)).join('')
  if ('$bytes' in spec) return new Uint8Array(spec.$bytes)
  if ('$arraybuffer' in spec) return new ArrayBuffer(spec.$arraybuffer)
  if ('number' in spec) return Number(spec.number)
  if ('bigint' in spec) return BigInt(spec.bigint)
  if ('fn' in spec) return () => 1
  if ('symbol' in spec) return Symbol(spec.symbol)
  if ('$error' in spec) {
    const e = new Error(spec.$error.message)
    e.stack = 'stack' in spec.$error ? spec.$error.stack : undefined
    return e
  }
  const out = {}
  for (const [k, v] of Object.entries(spec)) out[k] = decodeSpec(v)
  return out
}

/** One output, with a lone UTF-16 half (which JSON cannot hold for Rust) moved aside. */
function rec(input, output) {
  if (hasLone(output)) return { input, output: null, lone_surrogate: true, flare_output: stripLone(output) }
  return { input, output }
}

// ---------------------------------------------------------------- version.json

/** newerVersion lives in src/main/update.ts, which imports electron and cannot load here. Its
 *  text is cut out, types stripped, and written to a scratch module that is imported and removed. */
async function newerVersionFromSource() {
  const lines = readFileSync(join(PT, 'src/main/update.ts'), 'utf8').split(/\r?\n/)
  const start = lines.findIndex((l) => l.startsWith('export function newerVersion'))
  if (start < 0) throw new Error('newerVersion not found in src/main/update.ts')
  const end = lines.findIndex((l, i) => i > start && l === '}')
  const code = lines.slice(start, end + 1).join('\n').replace(/^export /, '')
  const dir = mkdtempSync(join(tmpdir(), 'oracle-newer-'))
  const file = join(dir, 'newer.mjs')
  writeFileSync(file, stripTypeScriptTypes(code) + '\nexport { newerVersion }\n')
  try {
    return (await import(pathToFileURL(file).href)).newerVersion
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

/** Flare's rule, written out here only to record the flagged answers; the Rust port is what is tested. */
const flareParts = (v) => v.replace(/^[vV]/, '').split('.').map((n) => (/^[0-9]+$/.test(n) ? Number(n) : 0))
function flareNewer(a, b) {
  const pa = flareParts(a)
  const pb = flareParts(b)
  for (let i = 0; i < 3; i += 1) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0)
    if (d !== 0) return d > 0
  }
  return false
}

function buildVersions(ptNewer) {
  const pairs = [
    ['0.7.1', '0.7.0'], ['0.8.0', '0.7.9'], ['1.0.0', '0.9.9'], ['0.7.0', '0.7.0'], ['0.6.9', '0.7.0'],
    ['v0.7.1', '0.7.0'], ['v0.7.0', 'v0.7.0'], ['nightly', '0.7.0'], ['', '0.7.0'],
    ['v1.2.3', '1.2.3'], ['1.2', '1.2.0'], ['0.10.0', '0.9.9'], ['1.2.3.4', '1.2.3'], ['x.y.z', '0.0.0'],
    ['0.0.0', '0.0.0'], ['V1.0.0', '0.9.0'],
  ]
  const flagged = [['1e2', '99'], ['0x10', '15'], [' 5 ', '4']]
  const rng = mulberry32(7)
  const v3 = () => `${Math.floor(rng() * 12)}.${Math.floor(rng() * 12)}.${Math.floor(rng() * 12)}`
  while (pairs.length < 60 - flagged.length) {
    const a = rng() < 0.2 ? 'v' + v3() : v3()
    pairs.push([a, v3()])
  }
  const rows = pairs.map(([a, b]) => ({ a, b, newer: ptNewer(a, b) }))
  for (const r of rows) {
    if (flareNewer(r.a, r.b) !== r.newer) throw new Error(`unflagged pair differs: ${r.a} vs ${r.b}`)
  }
  for (const [a, b] of flagged) {
    const newer = ptNewer(a, b)
    const alt = flareNewer(a, b)
    if (newer === alt) throw new Error(`flagged pair agrees: ${a} vs ${b}`)
    rows.push({ a, b, newer, flare: 'zero', flare_newer: alt })
  }
  return rows
}

// ---------------------------------------------------------------- notes.json

const GENERATED = [
  '<!-- Release notes generated using configuration in .github/release.yml at main -->',
  '',
  "## What's Changed",
  '* The update button opens a window with the patch notes by @Maxaubert in https://github.com/Maxaubert/PrismTerminal/pull/30',
  '* fix(ci): the bump edits four lockfile lines (#25) by @Maxaubert in https://github.com/Maxaubert/PrismTerminal/pull/26',
  '* Bump vite from 7.3.5 to 7.3.6 by @dependabot[bot] in https://github.com/Maxaubert/PrismTerminal/pull/31',
  '',
  '## New Contributors',
  '* @someone made their first contribution in https://github.com/Maxaubert/PrismTerminal/pull/31',
  '',
  '**Full Changelog**: https://github.com/Maxaubert/PrismTerminal/compare/v0.3.0...v0.4.0',
].join('\n')

const many = (n, f) => Array.from({ length: n }, (_, i) => f(i + 1)).join('\n')

/** The PT test titles, each a case with its title as `name` (Task 5 reads them by name). */
const NAMED = [
  ['is one plain entry per pull request, in order', GENERATED],
  ['drops the author and the url, and keeps the pull request number as text', GENERATED],
  ['does not print a number the title already carries', '* Links are painted (#10) by @Maxaubert in https://github.com/Maxaubert/PrismTerminal/pull/10'],
  ['drops the heading, the new contributors and the full changelog', GENERATED],
  ['reads Windows line endings the same way', GENERATED.replace(/\n/g, '\r\n')],
  ['takes a section that follows New Contributors back up', '## New Contributors\n* @a made their first contribution in https://x.test/pull/1\n## Fixes\n* A real fix'],
  ['strips an author with no url after it', '* Fix the thing by @someone'],
  ['keeps a title that names a handle and then says "in": only a url ends an author tail (a)', '* Mention people by @handle in comments'],
  ['keeps a title that names a handle and then says "in": only a url ends an author tail (b)', '* Mention people by @handle in comments by @me in https://x.test/pull/9'],
  ['keeps a "by" that is part of the title', '* Sort by name by @someone in https://x.test/pull/4'],
  ['undefined is one sensible line', { undefined: true }],
  ['null is one sensible line', null],
  ['an empty string is one sensible line', ''],
  ['whitespace is one sensible line', ' \n\t\r\n '],
  ['a number is one sensible line', 42],
  ['an object is one sensible line', { body: 'x' }],
  ['only boilerplate is one sensible line', "## What's Changed\n\n**Full Changelog**: https://x.test/compare/a...b"],
  ['only a comment is one sensible line', '<!-- nothing -->'],
  ['reads dashes, pluses, numbers and task boxes as bullets', '- one\n+ two\n1. three\n2) four\n- [x] five\n- [ ] six'],
  ['reads a plain paragraph line as an entry', 'This release fixes the prompt.\n\nAnd the tabs.'],
  ['reads a paragraph wrapped over several lines as ONE entry, and a bullet ends it', 'This release fixes\nthe prompt.\n* and a bullet\nthen prose again\n\nAnd the tabs.'],
  ['joins an indented continuation onto the bullet above it', '* a long\n  title wrapped\n* next'],
  ['drops headings and rules, whatever they say', '# Big\n### Small\n---\n***\n* kept'],
  ['collapses whitespace', '*   a \t  b    c  '],
  ['turns a markdown link into its text, never its target', '* See [the docs](https://evil.test/login) and [this](javascript:alert(1))'],
  ['turns an image into its alt text', '* Look ![a screenshot](https://x.test/a.png) here'],
  ['strips html, tags and comments alike', '* Hello <img src=x onerror="alert(1)"> <b>bold</b><!-- hidden\nacross lines --> <script>alert(2)</script> world'],
  ['is not fooled by a tag hidden inside a tag', '* a <scr<script>ipt>alert(1)</scr</script>ipt> b'],
  ['leaves a lone angle bracket alone: it is only text', '* a -> b, and x < y'],
  ['strips emphasis marks and code ticks, and keeps snake_case', '* **Bold** and `code` and ~~gone~~ and snake_case_name'],
  ['strips control characters and the marks that reorder text', '* safe\u202Eevil\u2066x\u2069\u200B\u0007\u0000 end'],
  ['never returns anything but strings, and never an empty one', '* \n* <b></b>\n* real\n* ``'],
  ['cuts a long entry, with an ellipsis, to the cap exactly', { concat: ['* ', { repeat: ['word ', 200] }] }],
  ['keeps the pull request number when it cuts the title', { concat: ['* ', { repeat: ['word ', 200] }, 'by @a in https://x.test/pull/77'] }],
  ['leaves an entry of exactly the cap alone', '* ' + 'x'.repeat(MAX_ENTRY_CHARS)],
  ['shows the first entries and counts the rest', many(MAX_ENTRIES + 7, (i) => `* change ${i}`)],
  ['counts nothing when the list fits exactly', many(MAX_ENTRIES, (i) => `* change ${i}`)],
  ['reads only the head of an enormous body, and quickly', { repeat: ['* line\n', 400000] }],
  ['does not hang on a line built to make a regex backtrack', { concat: ['* ', { repeat: ['[a](', 3000] }, ' by @', { repeat: ['a-', 3000] }, ' in ', { repeat: ['<', 3000] }] }],
]

function noteRecord(spec, name) {
  const r = parseReleaseNotes(decodeSpec(spec))
  if (r.entries.some((e) => hasLone(e))) throw new Error('an entry holds a lone surrogate: ' + JSON.stringify(spec))
  const out = {}
  if (name !== undefined) out.name = name
  out.body = spec
  out.entries = r.entries
  out.more = r.more
  out.empty = r.empty
  out.lens = r.entries.map((e) => e.length)
  return out
}

function buildNotes() {
  const rows = NAMED.map(([name, spec]) => noteRecord(spec, name))
  for (const e of EDGE) rows.push(noteRecord(e))
  const total = 450
  for (const b of bodies(total - rows.length, 20261018)) rows.push(noteRecord(b))
  if (rows.length !== total) throw new Error(`expected ${total} bodies, got ${rows.length}`)
  return rows
}

// ---------------------------------------------------------------- groups.json

function groupRows(notesRows) {
  const bodiesOut = notesRows.slice(0, 150).map((row) => {
    const r = parseReleaseNotes(decodeSpec(row.body))
    return { body: row.body, sections: groupReleaseNotes(r) }
  })
  const entries = TITLES.map((entry) => {
    const { kind, text } = classifyEntry(entry)
    const row = { entry, kind, text }
    if (/^terminal core:/i.test(entry) && kind === 'internal') row.flare = 'skip'
    return row
  })
  return { bodies: bodiesOut, entries }
}

// ---------------------------------------------------------------- flow.json

const OFFERS = {
  A: { version: '0.5.0', url: 'https://x.test/a.exe', notes: '* one', mock: false },
  B: { version: '0.5.1', url: '', notes: '', mock: true },
}
const FLOW_PCTS = [0, 33.4, -5, 250, NaN, Infinity, 50, 99.6, 100, 10]
const flowState = (s) => ({
  info: s.info ? { version: s.info.version, url: s.info.url, notes: s.info.notes, mock: s.info.mock } : null,
  phase: s.phase,
  pct: s.pct,
  open: s.open,
  notice: s.notice,
  aborting: s.aborting,
})
const flowAction = (a) => {
  switch (a.a) {
    case 'available':
      return { type: 'available', info: OFFERS[a.which] }
    case 'progress':
      return { type: 'progress', pct: Number(a.pct) }
    case 'settled':
      return { type: 'settled', ok: a.ok }
    default:
      return { type: a.a }
  }
}

function buildFlow() {
  const rng = mulberry32(11)
  const kinds = [
    () => ({ a: 'available', which: rng() < 0.5 ? 'A' : 'B' }),
    () => ({ a: 'open' }),
    () => ({ a: 'cancel' }),
    () => ({ a: 'hide' }),
    () => ({ a: 'install' }),
    () => {
      const v = FLOW_PCTS[Math.floor(rng() * FLOW_PCTS.length)]
      return { a: 'progress', pct: Number.isFinite(v) ? v : String(v) }
    },
    () => ({ a: 'abort' }),
    () => ({ a: 'settled', ok: true }),
    () => ({ a: 'settled', ok: false }),
    () => ({ a: 'dismiss' }),
  ]
  const sequences = []
  for (let s = 0; s < 200; s += 1) {
    const n = 3 + Math.floor(rng() * 23)
    const actions = []
    let state = NO_UPDATE
    const states = [flowState(state)]
    for (let i = 0; i < n; i += 1) {
      const act = kinds[Math.floor(rng() * kinds.length)]()
      actions.push(act)
      state = updateFlow(state, flowAction(act))
      states.push(flowState(state))
    }
    sequences.push({ actions, states })
  }
  return { offers: OFFERS, start: flowState(NO_UPDATE), sequences }
}

// ---------------------------------------------------------------- preview.json

async function runTrace(ms, cancelAfter) {
  const waits = []
  const onPct = []
  let calls = 0
  const wait = async (t) => {
    waits.push(t)
  }
  const cancelled = () => {
    calls += 1
    return cancelAfter !== null && calls > cancelAfter
  }
  const result = await runPreviewInstall((p) => onPct.push(p), { ms, wait, cancelled })
  return { ms, cancelAfter, waits, onPct, result }
}

async function buildPreview() {
  const parsed = parseReleaseNotes(SAMPLE_NOTES)
  const runs = []
  for (const [ms, cancelAfter] of [[3000, null], [1000, null], [0, null], [3000, 7], [3000, 20]]) {
    runs.push(await runTrace(ms, cancelAfter))
  }
  return {
    previewVersion: ['0.55.6', 'v1.2.3', 'x', '7', '0.0.0', '1.9', '', 'nightly', '3', '0.4.7', '1.19.3', 'v2.0.1'].map((i) => ({
      in: i,
      out: previewVersion(i),
    })),
    wantsPreview: [
      ['app.exe', '--preview-update'],
      ['app.exe', '--user-data-dir=x', '--e2e', 'main.js', '--preview-update', 'C:/f'],
      ['app.exe'],
      ['app.exe', '--preview-updates', 'preview-update', '--PREVIEW-UPDATE=1'],
      [],
      ['--preview'],
      ['--preview-update'],
      ['--preview-update', '--preview-update'],
      ['app.exe', '--previewupdate'],
      ['app.exe', ' --preview-update'],
    ].map((argv) => ({ argv, out: wantsPreview(argv) })),
    previewUpdate: previewUpdate('0.55.6'),
    previewMs: PREVIEW_MS,
    sample: { notes: SAMPLE_NOTES, parsed, grouped: groupReleaseNotes(parsed) },
    runs,
  }
}

// ---------------------------------------------------------------- gate.json

function buildGate() {
  const rng = mulberry32(31)
  const out = []
  for (let s = 0; s < 60; s += 1) {
    const g = createDiagGate(10000, 10)
    const ops = []
    const answers = []
    let now = 0
    const n = 40 + Math.floor(rng() * 41)
    for (let i = 0; i < n; i += 1) {
      now += Math.floor(rng() * (rng() < 0.2 ? 25000 : 900))
      if (rng() < 0.12) {
        ops.push({ op: 'sweep', now })
        answers.push(g.sweep(now))
      } else {
        const k = ['page-error', 'main-error', 'ipc-error'][Math.floor(rng() * 3)]
        const m = Math.floor(rng() * 15)
        const key = `${k}|boom ${m}`
        const line = { k, msg: `boom ${m}` }
        ops.push({ op: 'offer', k, key, line, now })
        answers.push(g.offer(k, key, { ...line }, now))
      }
    }
    out.push({ everyMs: 10000, budget: 10, ops, answers })
  }
  return out
}

// ---------------------------------------------------------------- summary.json

function buildSummary() {
  const capInputs = ['', 'abc', 'x'.repeat(300), 'x'.repeat(301), 'é'.repeat(301), 'a'.repeat(299) + '😀z', 'x'.repeat(2500)]
  const valueInputs = [
    null,
    { undefined: true },
    'str',
    'x'.repeat(301),
    5,
    { number: 'NaN' },
    { number: 'Infinity' },
    true,
    { bigint: '123' },
    { fn: true },
    { symbol: 's' },
    { $bytes: 4 },
    { $arraybuffer: 8 },
    [1, 2, 3],
    { a: 1, b: { c: 2 } },
    { a: { b: 1 } },
    Object.fromEntries(Array.from({ length: 14 }, (_, i) => [`k${i}`, i])),
    'a'.repeat(299) + '😀z',
  ]
  const argInputs = [
    { args: ['x'.repeat(301), 3, { $bytes: 9 }, [1, 2], { a: 1 }, null, 'typed text', 'secret'], opaque: false },
    { args: ['x'.repeat(301), 3, { $bytes: 9 }, [1, 2], { a: 1 }, null, 'typed text', 'secret'], opaque: true },
  ]
  const errInputs = [
    'plain string error',
    { $error: { message: 'm', stack: 's' } },
    { $error: { message: 'm' } },
    { message: 'x', stack: 'stack-text' },
    { message: 'x' },
    42,
    null,
    { $error: { message: 'm', stack: 'frame\n'.repeat(500) } },
  ]
  const cleanInputs = [
    { a: 'x', b: 1, c: null, d: true, e: { number: 'NaN' }, f: { number: 'Infinity' } },
    { stack: 'x'.repeat(2500), msg: 'x'.repeat(301) },
    Object.fromEntries(Array.from({ length: 45 }, (_, i) => [`k${i}`, i])),
    { list: [1, 2, { a: 1 }, [1, 2]], rec: { x: { y: { z: 1 } } }, long: 'é'.repeat(301) },
    { list: Array.from({ length: 12 }, (_, i) => i) },
    { inner: Object.fromEntries(Array.from({ length: 14 }, (_, i) => [`k${i}`, i])) },
    { text: 'a'.repeat(299) + '😀z' },
  ]
  const cap = capInputs.map((s) => rec(s, capString(s)))
  const value = valueInputs.map((v) => rec(v, summariseValue(decodeSpec(v))))
  const args = argInputs.map((c) => rec(c, summariseArgs(decodeSpec(c.args), c.opaque)))
  const errors = errInputs.map((e) => rec(e, errorFields(decodeSpec(e))))
  const clean = cleanInputs.map((f) => rec(f, cleanFields(decodeSpec(f))))
  return { capString: cap, summariseValue: value, summariseArgs: args, errorFields: errors, cleanFields: clean }
}

// ---------------------------------------------------------------- reader logs

const BASE = Date.parse('2026-10-17T09:00:00.000Z')
/** One line in the writer's shape: `t`, `up`, `src`, `k`, then the fields. */
const L = (off, src, k, fields = {}) => ({ t: new Date(BASE + off).toISOString(), up: off + 60000, src, k, ...fields })

const READER = {
  quiet: [
    L(0, 'main', 'session', { app: 'PrismTerminal', version: '0.55.6', pid: 4242, verbose: false }),
    L(1000, 'main', 'crumb', { a: 'tab-open', id: 't1', cwd: 'C:\\Users\\Admin', resume: false }),
    L(1200, 'main', 'crumb', { a: 'shell-spawn', id: 't1', pid: 5001, shell: 'pwsh', cwd: 'C:\\Users\\Admin', resume: false, ms: 180, warm: false }),
    L(300000, 'main', 'crumb', { a: 'memory', appMb: 210, gpuMb: 40, childrenMb: 60, workingSetMb: 310 }),
    L(3600000, 'main', 'crumb', { a: 'update-check', offered: false, fetchMs: 412, offerMs: 3, rssMb: 201 }),
    L(7200000, 'main', 'quit', { ms: 3 }),
  ],
  stall: [
    L(0, 'main', 'session', { app: 'PrismTerminal', version: '0.55.6', pid: 4343, verbose: true }),
    L(500, 'main', 'crumb', { a: 'tab-open', id: 't1', cwd: 'C:\\proj', resume: false }),
    L(1500, 'main', 'crumb', { a: 'tab-open', id: 't2', cwd: 'C:\\other', resume: false }),
    L(1600, 'main', 'crumb', { a: 'tab-switch', id: 't2' }),
    L(2000, 'main', 'crumb', { a: 'shell-spawn', id: 't1', pid: 5001, shell: 'pwsh', cwd: 'C:\\proj', resume: false, ms: 220, warm: false }),
    L(60000, 'main', 'crumb', { a: 'update-open', version: '0.56.0', preview: true }),
    L(70000, 'main', 'crumb', { a: 'tab-close', id: 't2', kind: 'closed' }),
    L(80000, 'page', 'mark', { note: 'the scroll froze' }),
    L(81000, 'page', 'page-stall', { ms: 910, blocking: 880, scripts: [{ fn: 'render', src: 'app.js', invoker: 'timer', ms: 880 }] }),
    L(82000, 'main', 'main-lag', { ms: 640, inflight: [{ ch: 'file:text', ms: 600, done: false }, { ch: 'tabs:save', ms: 30, done: true }] }),
    L(82500, 'main', 'ipc-slow', { ch: 'file:list', ms: 250, sync: true, args: ['C:\\Users', { x: 1 }] }),
    L(83000, 'main', 'ipc-error', { ch: 'shell:write', err: 'pipe closed' }),
    L(84000, 'page', 'page-stack', { ms: 1500, unresponsive: true, stack: 'at a (x.js:1)\n at b (y.js:2)' }),
    L(85000, 'page', 'page-error', { msg: 'Cannot read x', loc: 'app.js:10', repeats: 3, stack: 'Error: Cannot read x\n    at f (app.js:10)' }),
    L(86000, 'main', 'main-rejection', { msg: 'boom' }),
    L(87000, 'main', 'fs-slow', { ms: 1200, what: 'stat' }),
    L(88000, 'main', 'work-slow', { ms: 800, what: 'thumb' }),
    L(89000, 'main', 'logger-dropped', { n: 12 }),
    L(90000, 'main', 'crumb', { a: 'memory', appMb: 230, gpuMb: 44, childrenMb: 61, workingSetMb: 330 }),
    L(95000, 'main', 'gone', { reason: 'crashed' }),
    L(96000, 'main', 'crumb', { a: 'shell-exit', id: 't1', pid: 5001, exitCode: 0 }),
    L(100000, 'main', 'quit', { ms: 3 }),
  ],
  agent: [
    L(0, 'main', 'session', { app: 'PrismTerminal', version: '0.55.6', pid: 4444, verbose: false }),
    L(100, 'main', 'crumb', { a: 'tab-open', id: 't3', cwd: 'C:\\proj', resume: true }),
    L(120, 'main', 'crumb', { a: 'shell-spawn', id: 't3', pid: 5100, shell: 'pwsh', cwd: 'C:\\proj', resume: true, ms: 200, warm: false }),
    L(200, 'main', 'agent-hook', { id: 't3', state: 'working', kind: 'start' }),
    L(300, 'main', 'agent-title', { id: 't3', state: 'idle', agent: 'claude' }),
    L(400, 'main', 'agent-mark', { id: 't3', from: 'idle', to: 'working', held: ['idle'], why: 'hook start' }),
    L(900, 'main', 'agent-hook', { id: 't3', state: 'working', kind: 'tool', repeats: 2 }),
    L(2000, 'main', 'agent-mark', { id: 't3', from: 'working', to: 'done', held: ['working', 'done'], why: 'idle timeout', restored: 1500 }),
    L(2100, 'main', 'agent-hook', { id: 't3', state: 'idle', kind: 'stop', dropped: 4 }),
    L(5000, 'main', 'agent-restore', { id: 't3', cwd: 'C:\\proj' }),
    L(5100, 'main', 'crumb', { a: 'tab-open', id: 't4', cwd: 'C:\\other', resume: false }),
    L(5200, 'main', 'agent-hook', { id: 't4', state: 'working' }),
    L(5300, 'main', 'agent-title', { id: 't4', state: 'working', agent: 'codex' }),
    L(6000, 'main', 'crumb', { a: 'tab-close', id: 't4', kind: 'closed' }),
    L(6100, 'page', 'mark', { note: 'agent stuck' }),
    L(7000, 'main', 'quit', { ms: 3 }),
  ],
}

const READER_CASES = [
  { case: 'default', dir: 'reader/stall', args: [] },
  { case: 'all', dir: 'reader/stall', args: ['--all'] },
  { case: 'kinds', dir: 'reader/stall', args: ['--kinds', 'page-stall,main-lag'] },
  { case: 'n3', dir: 'reader/stall', args: ['--n', '3'] },
  { case: 'agent', dir: 'reader/agent', args: ['--agent'] },
  { case: 'agent-tab', dir: 'reader/agent', args: ['--agent', '--tab', 't3'] },
  { case: 'all-crumb', dir: 'reader/quiet', args: ['--all', '--kinds', 'crumb'] },
]

function buildReaderGoldens() {
  const diagMjs = join(PT, 'tools/diag.mjs')
  return READER_CASES.map((c) => {
    const args = ['--dir', c.dir, ...c.args]
    const stdout = execFileSync(process.execPath, [diagMjs, ...args], {
      cwd: FIX.diag,
      env: { ...process.env, TZ: 'UTC' },
      encoding: 'utf8',
    })
    return { case: c.case, args, stdout }
  })
}

// ---------------------------------------------------------------- main

const ptNames = readPtNames(PT)
const notes = buildNotes()
const versions = buildVersions(await newerVersionFromSource())

write(join(FIX.win, 'names.json'), { pt: ptNames })
write(join(FIX.version, 'version.json'), versions)
write(join(FIX.version, 'notes.json'), notes)
write(join(FIX.version, 'groups.json'), groupRows(notes))
write(join(FIX.version, 'flow.json'), buildFlow())
write(join(FIX.version, 'preview.json'), await buildPreview())
write(join(FIX.diag, 'summary.json'), buildSummary())
write(join(FIX.diag, 'gate.json'), buildGate())
for (const [name, lines] of Object.entries(READER)) {
  const dir = join(FIX.diag, 'reader', name)
  mkdirSync(dir, { recursive: true })
  writeFileSync(join(dir, 'diag.jsonl'), lines.map((l) => JSON.stringify(l)).join('\n') + '\n', 'utf8')
}
write(join(FIX.diag, 'reader_goldens.json'), buildReaderGoldens())
console.log(`oracle: ${versions.length} versions, ${notes.length} notes, ${TITLES.length} titles`)
