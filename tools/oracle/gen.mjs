// The oracle: runs Prism Terminal's own theme code under Node and writes the JSON fixtures the Rust tests read.
// Usage: node --no-warnings gen.mjs <out folder>      (after: node prep.mjs <pt export folder>)
import { mkdirSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { TERM_PRESETS } from './src/presets.ts'
import { resolvePreset, resolveCustom } from './resolve.mjs'
import { onGround } from './src/termGround.ts'
import { chromeTokens, CHROME_COLOUR_TOKENS } from './src/chromeTheme.ts'
import { orderTermThemes } from './src/themeOrder.ts'
import { linkColor } from './src/termLinks.ts'
import { sidebarGround } from './src/sidebarGround.ts'
import { RETIRED_THEMES } from './src/termThemeRetired.ts'
import { TERM_FONTS } from './src/fonts.ts'

const out = process.argv[2]
if (!out) throw new Error('usage: node gen.mjs <out folder>')
mkdirSync(out, { recursive: true })
const write = (name, value) => writeFileSync(join(out, name), JSON.stringify(value))

// 1. the presets as written, and each resolved
write('presets.json', TERM_PRESETS)
write('resolved_presets.json', Object.fromEntries(TERM_PRESETS.map((p) => [p.id, resolvePreset(p)])))

// 2. termTheme.snapshot.test: the same on picked grounds, and an opaque Custom whose cursor fails its floor
const OPAQUE_CUSTOM = {
  bg: '#1a1b26', fg: '#c0caf5', cursor: '#20212c',
  ansi: { black: '#15161e', red: '#f7768e', green: '#2a2b36', yellow: '#e0af68', blue: '#7aa2f7', magenta: '#bb9af7', cyan: '#7dcfff', white: '#a9b1d6' }
}
const onGrounds = {}
for (const ground of ['#f4f1ea', '#203040'])
  for (const p of TERM_PRESETS) onGrounds[`${p.id}@${ground}`] = onGround(resolvePreset(p), ground)
for (const ground of ['#f4f1ea', '#203040']) onGrounds[`custom@${ground}`] = onGround(resolveCustom(OPAQUE_CUSTOM), ground)
write('on_grounds.json', onGrounds)
write('custom_opaque.json', { input: OPAQUE_CUSTOM, resolved: resolveCustom(OPAQUE_CUSTOM) })

// 3. chrome tokens: every preset, the arguments chromeTheme.opacity.test snapshots
const toAlpha = (n) => Math.round((n / 100) * 255) / 255
const chrome = {}
for (const p of TERM_PRESETS) {
  const t = resolvePreset(p)
  chrome[p.id] = {
    theme: chromeTokens(t, 1, p.accent).vars,
    glass: chromeTokens(t, toAlpha(60), p.accent).vars,
    picked: chromeTokens(t, 1, p.accent, 'hairline', '#e07a2f').vars,
    pickedGlass: chromeTokens(t, toAlpha(60), p.accent, 'hairline', '#e07a2f').vars
  }
}
write('chrome_presets.json', chrome)
const alphaSweep = {}
for (const id of ['pt-default', 'prism', 'paper', 'dracula']) {
  const p = TERM_PRESETS.find((x) => x.id === id)
  alphaSweep[id] = {}
  for (let n = 30; n <= 100; n += 1) alphaSweep[id][n] = chromeTokens(resolvePreset(p), toAlpha(n)).vars
}
write('chrome_alpha_sweep.json', alphaSweep)

// 4. odds and ends with a measured answer
write('wall_order.json', orderTermThemes(TERM_PRESETS).map((p) => p.id))
write('link_colours.json', Object.fromEntries(TERM_PRESETS.map((p) => {
  const t = resolvePreset(p)
  return [p.id, linkColor(t.background, t.foreground)]
})))
write('sidebar_grounds.json', Object.fromEntries(TERM_PRESETS.map((p) => [p.id, sidebarGround(p.bg)])))
write('chrome_token_names.json', CHROME_COLOUR_TOKENS)
write('retired.json', RETIRED_THEMES)
write('fonts.json', TERM_FONTS)
console.log('wrote', out)
