// Parses a vitest .snap file (pretty-format output of plain objects) into { name: value } and compares it with the oracle.
// Usage: node --no-warnings snapcheck.mjs <pt export folder>
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

export function parseSnap(text) {
  const out = {}
  const re = /exports\[`([^`]+)`\] = `\r?\n([\s\S]*?)\r?\n`;/g
  let m
  while ((m = re.exec(text))) {
    const json = m[2].replace(/,(\s*[}\]])/g, '$1') // pretty-format leaves trailing commas
    out[m[1]] = JSON.parse(json)
  }
  return out
}

const pt = process.argv[2]
const j = (n) => JSON.parse(readFileSync(new URL('./out/' + n, import.meta.url)))
const same = (a, b) => JSON.stringify(sortKeys(a)) === JSON.stringify(sortKeys(b))
function sortKeys(v) {
  if (Array.isArray(v)) return v.map(sortKeys)
  if (v && typeof v === 'object') return Object.fromEntries(Object.keys(v).sort().map((k) => [k, sortKeys(v[k])]))
  return v
}

// termTheme.snapshot.test
const s1 = parseSnap(readFileSync(join(pt, 'core/renderer/lib/__snapshots__/termTheme.snapshot.test.ts.snap'), 'utf8'))
const k1 = Object.keys(s1)
console.log('termTheme.snapshot entries:', k1)
const resolved = j('resolved_presets.json')
console.log('every preset equal:', same(s1[k1.find((k) => k.includes('every preset 1'))], resolved))
console.log('opaque custom equal:', same(s1[k1.find((k) => k.includes('failing cursor'))], j('custom_opaque.json').resolved))
const grounds = s1[k1.find((k) => k.includes('picked ground'))]
const mine = j('on_grounds.json')
console.log('picked grounds: snap keys', Object.keys(grounds).length, 'oracle keys', Object.keys(mine).length, 'equal:', same(grounds, mine))

// chromeTheme.opacity.test
const s2 = parseSnap(readFileSync(join(pt, 'src/renderer/src/lib/__snapshots__/chromeTheme.opacity.test.ts.snap'), 'utf8'))
const k2 = Object.keys(s2)
console.log('opacity snapshot entries:', k2.length, k2.slice(0, 3))
const sweep = j('chrome_alpha_sweep.json')
let bad = 0
for (const id of ['pt-default', 'prism', 'dracula', 'paper']) {
  const key = k2.find((k) => k.includes(`${id}: every N`))
  const snap = s2[key]
  for (let n = 30; n <= 100; n += 1) {
    const { ['--p-accent-solid']: _a, ['--p-switch-knob']: _k, ...rest } = sweep[id][n]
    if (!same(rest, snap[n])) { bad += 1; if (bad < 4) console.log('DIFF', id, n) }
  }
}
console.log('alpha sweep diffs:', bad)
const chrome = j('chrome_presets.json')
let bad2 = 0
for (const [id, v] of Object.entries(chrome)) {
  const key = k2.find((k) => k.endsWith(`> ${id} 1`))
  const snap = s2[key]
  for (const part of ['theme', 'glass', 'picked', 'pickedGlass']) {
    const { ['--p-accent-solid']: _a, ['--p-switch-knob']: _k, ...rest } = v[part]
    if (!same(rest, snap[part])) { bad2 += 1; if (bad2 < 4) console.log('DIFF', id, part) }
  }
}
console.log('preset chrome diffs:', bad2, 'of', Object.keys(chrome).length * 4)
