// Copies the pure Prism Terminal theme modules flat into ./src and rewrites their import specifiers so
// Node's built-in type stripping can run them. Usage: node prep.mjs <path to the PT v0.55.6 export>
import { mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs'
import { basename, join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const pt = process.argv[2]
if (!pt) throw new Error('usage: node prep.mjs <pt export folder>')
const here = dirname(fileURLToPath(import.meta.url))
const out = join(here, 'src')
rmSync(out, { recursive: true, force: true })
mkdirSync(out, { recursive: true })

const FILES = [
  'core/renderer/lib/termAnsi',
  'core/renderer/lib/colour',
  'core/renderer/lib/oklab',
  'core/renderer/lib/switchKnob',
  'core/renderer/lib/nameInk',
  'core/renderer/lib/markColours',
  'core/renderer/lib/markPalette',
  'core/renderer/lib/seeThrough',
  'core/renderer/lib/themeOrder',
  'core/renderer/lib/termGround',
  'core/renderer/lib/termLinks',
  'core/renderer/lib/termThemeRetired',
  'core/shared/ground',
  'src/renderer/src/lib/chromeTheme',
  'src/renderer/src/lib/sidebarGround',
  'src/shared/windowEdges'
]
// every import of a sibling module, however PT spelled the path, becomes './<name>.ts'
const SPEC = /from '(?:\.\/|\.\.\/|\.\.\/\.\.\/|@core\/renderer\/lib\/|@core\/shared\/|@shared\/|\.\.\/\.\.\/shared\/)([A-Za-z]+)'/g
for (const f of FILES) {
  const src = readFileSync(join(pt, f + '.ts'), 'utf8').replace(SPEC, (_m, name) => `from './${name}.ts'`)
  writeFileSync(join(out, basename(f) + '.ts'), src)
}
// termTheme.ts needs the host and the stores; the oracle uses this trimmed copy: TERM_PRESETS and the two resolvers only.
const tt = readFileSync(join(pt, 'core/renderer/lib/termTheme.ts'), 'utf8')
const start = tt.indexOf('export const TERM_PRESETS')
const end = tt.indexOf('/** What an unknown id')
writeFileSync(
  join(out, 'presets.ts'),
  `import { DEFAULT_GROUND } from './ground.ts'\nexport interface TermPreset { id: string; name: string; bg: string; fg: string; cursor: string; accent?: string; ansi?: Record<string, string> }\n` +
    tt.slice(start, end)
)
// termLook.ts pulls in the host and the stores; the oracle takes only its font table.
const tl = readFileSync(join(pt, 'core/renderer/lib/termLook.ts'), 'utf8')
const fStart = tl.indexOf('export const TERM_FONTS')
const fEnd = tl.indexOf('] as const', fStart) + '] as const'.length
writeFileSync(join(out, 'fonts.ts'), tl.slice(fStart, fEnd) + '\n')
console.log('prepared', FILES.length + 2, 'files in', out)
