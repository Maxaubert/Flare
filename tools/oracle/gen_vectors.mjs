// Differential vectors for the pure colour maths: Rust must answer these exactly.
// Usage: node --no-warnings gen_vectors.mjs <out folder>
import { mkdirSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { TERM_PRESETS } from './src/presets.ts'
import { resolvePreset, resolveCustom } from './resolve.mjs'
import { contrastRatio, deriveAnsi, ensureContrast, mixHex, normalizeColor } from './src/termAnsi.ts'
import { colourCommit, composite, format, inkOn, legibleOn, legibleOnStrict, parseColour, rgbToHsl, selectionFor, toStored, withAlpha } from './src/colour.ts'
import { onGround } from './src/termGround.ts'
import { chromeTokens } from './src/chromeTheme.ts'
import { floorMark, rainbowOn, ICON_RAINBOW } from './src/markColours.ts'
import { markPalette } from './src/markPalette.ts'
import { nameInk, rainbowInk } from './src/nameInk.ts'
import { switchKnob } from './src/switchKnob.ts'
import { seeThroughAlpha } from './src/seeThrough.ts'
import { toLab, fromLab, liftToFloor } from './src/oklab.ts'
import { EDGE_BORDER_STEP } from './src/windowEdges.ts'

const out = process.argv[2]
if (!out) throw new Error('usage: node gen_vectors.mjs <out folder>')
mkdirSync(out, { recursive: true })
const write = (name, value) => writeFileSync(join(out, name), JSON.stringify(value))

// PT's own seeded generator (termAnsi.test "property check")
let seed = 42
const rnd = () => {
  seed = (seed * 1103515245 + 12345) % 2147483648
  return seed / 2147483648
}
const hexRnd = () => '#' + [0, 0, 0].map(() => Math.floor(rnd() * 256).toString(16).padStart(2, '0')).join('')
const ansiVectors = []
for (let i = 0; i < 200; i += 1) {
  const bg = hexRnd()
  const fg = ensureContrast('#c8c8c8', bg, 4.5)
  ansiVectors.push({ bg, fg, ansi: deriveAnsi(bg, fg) })
}
write('ansi_vectors.json', ansiVectors)

const parseInputs = ['#abc', 'abc', '#abcd', '#AABBCC', 'aabbcc80', 'rgb(1, 2, 3)', 'rgba(1,2,3,0.5)', 'rgba(1, 2, 3, 50%)', 'rgb(1 2 3 / 0.25)', 'RGBA(1 2 3 / 25%)',
  'hsl(0, 100%, 50%)', 'hsl(120deg 100% 25%)', 'hsla(240, 100%, 50%, 0.5)', 'hsl(240 100% 50% / 50%)', '', '   ', '#12345', '#1234567', 'rgb(300,0,0)', 'rgb(1,2)',
  'rgba(1,2,3,2)', 'hsl(200, 50, 40)', 'hsl(200, 50%, 40)', 'red', 'rgb(1,2,3', '#ggg', '#3b82f680', 'hsla(217, 91%, 60%, 0.5)']
write('parse_vectors.json', parseInputs.map((t) => {
  const c = parseColour(t)
  return { text: t, parsed: c, stored: c ? toStored(c) : null, hex: c ? format(c, 'hex') : null, rgba: c ? format(c, 'rgba') : null, hsla: c ? format(c, 'hsla') : null }
}))

const grounds = ['#0b0b0f', '#ffffff', '#3c3c64', '#808080', '#101010', '#000000', '#f6f4ee']
const picks = ['#e05561', '#d7dae1', '#4aa5f0', '#222222', '#fafafa', '#ec9448']
const legible = []
for (const g of grounds) for (const p of picks) for (let a = 1; a <= 10; a += 1) {
  const c = withAlpha(p, a / 10)
  legible.push({ c, g, floor45: legibleOn(c, g, 4.5), floor3: legibleOn(c, g, 3), strict3: legibleOnStrict(c, g, 3), ink: inkOn(c, g), composite: composite(c, g) })
}
write('legible_vectors.json', legible)

const selection = []
for (const accent of ['#4682fb', '#ec9448', '#5b5bd6', '#a6e22e', '#fe8f34'])
  for (const a of [0.4, 0.6, 0.8, 1])
    for (const gs of [['#0b0b0f', '#1b1d24'], ['#000000', '#f5f5f5'], ['#171717', '#1f1f1e']])
      selection.push({ fill: withAlpha(accent, a), grounds: gs, out: selectionFor(withAlpha(accent, a), gs) })
write('selection_vectors.json', selection)

write('commit_vectors.json', [
  ['#AABBCC', '#aabbcc'], ['abc', '#aabbcc'], ['rgb(170, 187, 204)', '#aabbcc'], ['#aabbccff', '#aabbcc'], ['#aabbcc80', '#aabbcc'], ['zz', '#aabbcc'],
  ['#203040', '#0b0d12cb'], ['203040', '#0b0d12cb'], ['rgb(32, 48, 64)', '#0b0d1280'], ['#203040ff', '#0b0d12cb'], ['#20304080', '#0b0d12cb'], ['rgba(32, 48, 64, 1)', '#0b0d1280']
].map(([d, v]) => ({ draft: d, value: v, out: colourCommit(d, v) })))

const onG = {}
for (const g of ['#f4f1e8', '#101010', '#1d3fbf']) for (const p of TERM_PRESETS) onG[`${p.id}@${g}`] = onGround(resolvePreset(p), g)
write('on_ground_vectors.json', onG)

const KEYS = ['black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white', 'brightBlack', 'brightRed', 'brightGreen', 'brightYellow', 'brightBlue', 'brightMagenta', 'brightCyan', 'brightWhite']
const custom = {}
for (const p of TERM_PRESETS) {
  const base = resolvePreset(p)
  for (let i = 1; i <= 9; i += 1) {
    const a = i / 10
    custom[`${p.id}@${i}`] = resolveCustom({ bg: p.bg, fg: withAlpha(p.fg, a), cursor: withAlpha(p.cursor, a), ansi: Object.fromEntries(KEYS.map((k) => [k, withAlpha(base[k], a)])) })
  }
}
write('custom_alpha_vectors.json', custom)

const rg = {
  volt: ['#050706', mixHex('#050706', '#eef2e6', 0.04), mixHex('#050706', '#eef2e6', 0.11)],
  paper: ['#f6f4ee', mixHex('#f6f4ee', '#2a2620', 0.04), mixHex('#f6f4ee', '#2a2620', 0.11)],
  cinder: ['#383c44']
}
write('mark_vectors.json', {
  rainbow: ICON_RAINBOW,
  floored: Object.fromEntries(Object.entries(rg).map(([k, gs]) => [k, { rainbowOn: rainbowOn(gs), each: ICON_RAINBOW.map((c) => floorMark(c, gs, 3)) }])),
  palettes: {
    volt: [true, false].map((rb) => markPalette({ colours: { working: '#d8ff26', finished: '#22c55e', question: '#3b82f6', failed: '#ff3b5c' }, grounds: ['#050706'], solidGround: '#050706', text: '#eef2e6', rainbow: rb })),
    paper: [true, false].map((rb) => markPalette({ colours: { working: '#3f8ccb', finished: '#1b9a49', question: '#3b82f6', failed: '#e05561' }, grounds: ['#f6f4ee'], solidGround: '#f6f4ee', text: '#2a2620', rainbow: rb }))
  },
  nameInk: [['#d8ff26', '#eef2e6', true], ['#3b82f6', '#eef2e6', true], ['#cfe4ff', '#eef2e6', true], ['#2a2c30', '#1d1f1a', false], ['#f0f0f0', '#ffffff', true], ['#f0f0f0', '#ffffff', false]].map(([f, t, d]) => ({ fill: f, text: t, dark: d, ink: nameInk(f, t, d) })),
  rainbowInk: [['#eef2e6', true], ['#2a2620', false], ['#808080', true]].map(([t, d]) => ({ text: t, dark: d, ink: rainbowInk(t, d) })),
  switchKnob: [['#fe8f34', true], ['#d8ff26', true], ['#3a73a0', false], ['#cfe4ff', false], ['#2a2c30', true]].map(([t, d]) => ({ track: t, dark: d, ink: switchKnob(t, d) })),
  seeThrough: ['#0b0b0f', '#ffffff', '#aaaaaa', '#a9a9a9', '#ffffff20', '#00000020', 'soup'].map((g) => ({ ground: g, alpha: seeThroughAlpha(g) }))
})

write('oklab_vectors.json', ['#000000', '#ffffff', '#808080', '#ff0000', '#171717', '#fe8f34', '#140707', '#f6f4ee'].map((h) => ({ hex: h, lab: toLab(h), back: fromLab(toLab(h)), lift: liftToFloor(h, ['#121212'], 3), liftLight: liftToFloor(h, ['#f6f4ee'], 3) })))

// src/main/windowEdge.ts edgeFor, with material.ts validBg and isLightBg (copied here: those files import Electron)
const validBg = (h) => typeof h === 'string' && /^#[0-9a-f]{6}$/i.test(h)
const isLightBg = (hex) => { const n = parseInt(hex.slice(1), 16); return (0.2126 * (n >> 16) + 0.7152 * ((n >> 8) & 255) + 0.0722 * (n & 255)) / 255 > 0.5 }
const edgeFor = (bg, edges = 'hairline') => {
  const steps = EDGE_BORDER_STEP[edges] === undefined ? EDGE_BORDER_STEP.hairline : EDGE_BORDER_STEP[edges]
  if (!steps) return 'none'
  const hex = validBg(bg) ? bg : '#0b0b0f'
  const n = parseInt(hex.slice(1), 16)
  const to = isLightBg(hex) ? 0 : 255
  const step = isLightBg(hex) ? steps.light : steps.dark
  const mix = (c) => Math.round(c + (to - c) * step)
  return `#${((mix((n >> 16) & 255) << 16) | (mix((n >> 8) & 255) << 8) | mix(n & 255)).toString(16).padStart(6, '0')}`
}
write('edge_vectors.json', ['#0b0b0f', '#f4f4f4', '#171717', '#ffffff', '#000000', '#e4e4e2', 'javascript:1'].flatMap((bg) => ['hairline', 'faint', 'solid', 'none'].map((e) => ({ bg, edges: e, out: edgeFor(bg, e) }))))

const preset = (id) => TERM_PRESETS.find((x) => x.id === id)
write('chrome_edges.json', Object.fromEntries(['pt-default', 'paper', 'dracula'].map((id) => [id, Object.fromEntries(['hairline', 'faint', 'solid', 'none'].map((e) => [e, chromeTokens(resolvePreset(preset(id)), 1, preset(id).accent, e).vars]))])))

// the see-through slider: paintChrome hands chromeTokens the ground alpha 1 - level / 100 (termLook.ts termGroundAlpha)
const levels = {}
for (const id of ['pt-default', 'paper', 'volt', 'dracula']) {
  levels[id] = {}
  for (const level of [0, 1, 10, 25, 30, 50, 69, 70]) levels[id][level] = chromeTokens(resolvePreset(preset(id)), 1 - level / 100, preset(id).accent).vars
}
write('chrome_levels.json', levels)

const t0 = resolvePreset(preset('pt-default'))
const picksRows = []
for (const accent of [null, '#ff0000', '#3b78ff', '#fe8f34', '#f5d90a', '#140707', '#0a0a0a', '#f7f7f7', '#e07a2f80', '#cfe4ff80', '#2a2c3080'])
  for (const secondary of [null, '#3a3a3a'])
    for (const alpha of [1, 153 / 255])
      for (const bgPick of [null, '#f4f1ea'])
        picksRows.push({ accent, secondary, alpha, bg: bgPick, vars: chromeTokens(bgPick ? { ...t0, background: bgPick } : t0, alpha, '#fe8f34', 'hairline', accent, secondary).vars })
write('chrome_picks.json', picksRows)
// src/renderer/src/lib/agentColors.ts themeAgentColors and marksApart, with the stores replaced by arguments
// (that file imports the stores; the logic below is copied from it unchanged).
const FALLBACK_DONE = '#22c55e'
const FLOOR = 3
const FALLBACK_FAIL_ALT = '#d946ef'
const QUESTION_BLUE = '#3b82f6'
const FAILED_RED = '#ef4444'
const rgbOf = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16) / 255)
function hueSat(h) {
  const c = parseColour(h)
  if (!c) return { hue: 0, sat: 0 }
  const { h: hue, s } = rgbToHsl(c)
  return { hue, sat: s / 100 }
}
function marksApart(a, b) {
  const [x, y] = [hueSat(a), hueSat(b)]
  const turn = Math.abs(x.hue - y.hue)
  if (x.sat > 0.25 && y.sat > 0.25 && Math.min(turn, 360 - turn) >= 20) return true
  const [p, q] = [rgbOf(a), rgbOf(b)]
  return Math.hypot(p[0] - q[0], p[1] - q[1], p[2] - q[2]) * 255 >= 80
}
function themeAgentColors(p, background, windowAccent) {
  const own = resolvePreset(p)
  const theme = background ? { ...own, background } : own
  const vars = chromeTokens(theme, 1, p.accent, undefined, windowAccent).vars
  const bg = vars['--p-bg-solid']
  const working = vars['--p-accent-solid']
  const green = ensureContrast(normalizeColor(theme.green ?? '', FALLBACK_DONE), bg, FLOOR)
  const finished = contrastRatio(green, working) < 1.15 ? ensureContrast(FALLBACK_DONE, bg, FLOOR) : green
  const question = ensureContrast(QUESTION_BLUE, bg, FLOOR)
  const tries = [theme.red, FAILED_RED, theme.magenta, FALLBACK_FAIL_ALT].map((c) => ensureContrast(normalizeColor(c ?? '', FAILED_RED), bg, FLOOR))
  const failed = tries.find((c) => marksApart(c, working) && marksApart(c, finished)) ?? tries[0]
  return { working, finished, question, failed }
}
const agent = {}
for (const p of TERM_PRESETS) {
  agent[p.id] = themeAgentColors(p, null, null)
  agent[`${p.id}@bg`] = themeAgentColors(p, '#f4f1ea', null)
  agent[`${p.id}@accent`] = themeAgentColors(p, null, '#e07a2f80')
}
write('agent_colours.json', agent)
write('marks_apart.json', [['#ff615a', '#fe8f34'], ['#ef4444', '#e5484d'], ['#22c55e', '#ef4444'], ['#777777', '#7a7a7a']].map(([a, b]) => ({ a, b, apart: marksApart(a, b) })))
console.log('wrote vectors to', out)
