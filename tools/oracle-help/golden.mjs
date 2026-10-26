#!/usr/bin/env node
// Writes the golden vectors that the Rust search and tokeniser must match,
// made by Prism Terminal's own search.ts (copied into ./src, unchanged apart
// from the import specifiers). Run after copy-sources.mjs and dump.mjs:
//   node golden.mjs
// Outputs (crates/flare-help/tests/fixtures):
//   golden_search.json  every query, its shell and limit, PT's ids and scores
//   words.json          tokenise, meaningfulWords, stem, expand, withinOneEdit

import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { ALL_HELP } from './src/index.ts'
import { FLARE_AGENTS_HELP, FLARE_ALL_HELP } from './flare.mjs'
import { expand, meaningfulWords, searchHelp, stem, tokenise, withinOneEdit } from './src/search.ts'
import { FIXTURE_DIR, EM_DASH } from './dump.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
const QUERIES = JSON.parse(readFileSync(join(HERE, 'queries.json'), 'utf8'))

// Strings for the tokeniser: accents, apostrophes, case, symbols, empties,
// a long word, emoji and digits. Written here, never in the output's own words.
const TOKENISE_INPUTS = [
  '',
  ' ',
  'Dirèctory',
  'DIRECTORY',
  "what's",
  'don’t',
  "don't",
  'ʼtick',
  "it's a `tick`",
  'Get-ChildItem',
  'get-childitem -Recurse',
  'ls -la',
  'dir /a',
  'rd /s /q',
  'C:\\Users\\Admin',
  '/compact',
  '--force',
  '-m "MESSAGE"',
  'a.b.c',
  'tar.gz',
  'ctrl+c',
  'Ctrl+Shift+V',
  'F5',
  '10 MB',
  '3000',
  'port 3000',
  'user@example.com',
  'naïve café',
  'Ångström',
  'Zürich',
  'Ελληνικά',
  'Привет',
  'こんにちは',
  'emoji 🙂 here',
  'x'.repeat(50),
  'x'.repeat(41),
  'running processes',
  'running',
  'primitive',
  'xyzzy',
  'delete',
  'delete files',
  'delete  files',
  'tab\tseparated',
  'new\nline',
  '   leading and trailing   ',
  '((brackets))',
  '[square]{curly}',
  'semi;colon:colon',
  'pipe|and&amp',
  '$env:PATH',
  '%USERPROFILE%',
  'proccess',
  'direcotry',
  'biggest files',
  'big file',
  'folders',
  'folder',
  'directories',
  'directory',
  'undoing commits',
  'undo',
  'where am I?',
  'how do I find big files',
  'the a an',
  'RUN AS ADMIN',
  'git-status',
  '/comp',
  'cd -',
  'ps-biggest-files'
]

// The synonym table is not exported by search.ts, so its literal is read from
// the copy: each inner array of single-quoted words is one group. Parsed as
// text, never evaluated.
function synonymGroups() {
  const src = readFileSync(join(HERE, 'src', 'search.ts'), 'utf8')
  const m = src.match(/const SYNONYM_GROUPS:[^=]*=\s*(\[[\s\S]*?\n\])/)
  if (!m) throw new Error('SYNONYM_GROUPS not found in src/search.ts')
  return [...m[1].matchAll(/\[([^\]]*)\]/g)].map((g) => [...g[1].matchAll(/'([^']*)'/g)].map((w) => w[1]))
}

/** Every string a catalogue entry carries, for the vocabulary. */
function catalogueTexts() {
  const texts = []
  for (const e of FLARE_ALL_HELP) {
    texts.push(e.task, e.summary, e.command, ...e.keywords, ...Object.keys(e.placeholders ?? {}))
    for (const v of e.variants ?? []) texts.push(v.label, v.command, ...(v.keywords ?? []))
  }
  return texts
}

/** Each query run as PT's search runs it. Generated queries come from one
 *  entry in three of Prism Terminal's, in catalogue order, and from every one
 *  of Flare's (flare.mjs), each run in the entry's own shell. */
function searchCases() {
  const cases = []
  for (const q of QUERIES) {
    for (const shell of ['powershell', 'cmd', 'bash']) cases.push({ q, shell, limit: 50 })
    cases.push({ q, shell: null, limit: 10 })
  }
  const fromEntry = (e) => {
    const own = e.shell === 'any' || e.shell === 'windows' ? 'powershell' : e.shell
    const qs = [
      e.task,
      `how do i ${e.task.toLowerCase()}`,
      ...e.keywords.slice(0, 3),
      ...(e.variants ?? []).map((v) => v.label),
      e.command.split(' ')[0]
    ]
    const longest = e.task.split(/[^A-Za-z]+/).reduce((a, b) => (b.length > a.length ? b : a), '')
    if (longest.length >= 5) qs.push(longest.slice(0, 2) + longest.slice(3))
    for (const q of qs) cases.push({ q, shell: own, limit: 10 })
  }
  ALL_HELP.forEach((e, i) => {
    if (i % 3 === 0) fromEntry(e)
  })
  FLARE_AGENTS_HELP.forEach(fromEntry)
  return cases
}

function golden() {
  const cases = searchCases()
  const rows = cases.map((c) => {
    const opts = c.shell ? { shell: c.shell, limit: c.limit } : { limit: c.limit }
    const hits = searchHelp(FLARE_ALL_HELP, c.q, opts)
    return {
      q: c.q,
      shell: c.shell,
      limit: c.limit,
      ids: hits.map((h) => h.entry.id),
      scores: hits.map((h) => h.score)
    }
  })
  const text = `{"queries": [\n${rows.map((r) => JSON.stringify(r)).join(',\n')}\n]}\n`
  return { count: rows.length, text }
}

function words() {
  const groups = synonymGroups()
  const vocab = [...new Set(catalogueTexts().flatMap((t) => tokenise(t)))].sort()
  const stems = Object.fromEntries(vocab.map((w) => [w, stem(w)]))

  const groupWords = [...new Set(groups.flat())]
  const expandKeys = [...new Set([...groupWords, 'running', 'primitive', 'xyzzy'])].sort()
  const expanded = Object.fromEntries(expandKeys.map((w) => [w, expand(w)]))

  const meaningful = QUERIES.slice(0, 80).map((q) => [q, meaningfulWords(q)])

  // 200 pairs from the catalogue's own words (four letters or more), cycling
  // through: equal, one wrong letter, a swap, a missing letter, an extra
  // letter, two slips, and a length difference of two.
  const pool = vocab.filter((w) => w.length >= 4)
  const edit = []
  for (let i = 0; edit.length < 200; i += 1) {
    const w = pool[(i * 7919) % pool.length]
    const idx = (i * 3) % w.length
    const at = (s, p, c) => s.slice(0, p) + c + s.slice(p + 1)
    let b
    switch (i % 7) {
      case 0:
        b = w
        break
      case 1:
        b = at(w, idx, w[idx] === 'x' ? 'y' : 'x')
        break
      case 2: {
        const p = Math.min(idx, w.length - 2)
        b = w.slice(0, p) + w[p + 1] + w[p] + w.slice(p + 2)
        break
      }
      case 3:
        b = w.slice(0, idx) + w.slice(idx + 1)
        break
      case 4:
        b = w.slice(0, idx) + 'q' + w.slice(idx)
        break
      case 5:
        b = at(at(w, idx, w[idx] === 'x' ? 'y' : 'x'), (idx + 2) % w.length, 'z')
        break
      default:
        b = `${w}zz`
    }
    edit.push([w, b, withinOneEdit(w, b)])
  }

  const tokenised = TOKENISE_INPUTS.map((s) => [s, tokenise(s)])
  const out = { tokenise: tokenised, meaningful, stem: stems, expand: expanded, edit }
  return { out, groupCount: groups.length, vocab: vocab.length }
}

function main() {
  mkdirSync(FIXTURE_DIR, { recursive: true })

  const g = golden()
  if (g.text.includes(EM_DASH)) throw new Error('em-dash in golden_search.json')
  writeFileSync(join(FIXTURE_DIR, 'golden_search.json'), g.text)

  const w = words()
  const wordsText = `${JSON.stringify(w.out, null, 1)}\n`
  if (wordsText.includes(EM_DASH)) throw new Error('em-dash in words.json')
  writeFileSync(join(FIXTURE_DIR, 'words.json'), wordsText)

  const mb = (n) => (n / 1048576).toFixed(2)
  console.log(`golden queries: ${g.count} (${mb(g.text.length)} MB)`)
  console.log(`vocabulary: ${w.vocab} words, synonym groups: ${w.groupCount}`)
  console.log(`words.json: ${mb(wordsText.length)} MB`)
  if (g.text.length > 4 * 1048576) console.log('golden is over 4 MB: sample every fourth entry and say so in the README')
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) main()
