// termTheme.ts resolveSavedTheme and resolveCustomTheme, as written there, with the host and store branches removed.
import { deriveAnsi, legiblePalette } from './src/termAnsi.ts'
import { composite, legibleOn, opaque, parseColour } from './src/colour.ts'
import { DEFAULT_GROUND } from './src/ground.ts'

export function resolvePreset(p) {
  return {
    background: p.bg,
    foreground: p.fg,
    cursor: p.cursor,
    selectionBackground: `${p.cursor}55`,
    ...legiblePalette(p.ansi ?? deriveAnsi(p.bg, p.fg), p.bg)
  }
}

export function resolveCustom(c, ground) {
  const own = opaque(c.bg, DEFAULT_GROUND)
  const g = ground && parseColour(ground) ? opaque(ground) : own
  const cursor = legibleOn(c.cursor, g, 3)
  const ansi = {}
  for (const [k, v] of Object.entries(c.ansi)) ansi[k] = composite(v, g)
  return {
    background: c.bg,
    foreground: legibleOn(c.fg, g, 4.5),
    cursor,
    selectionBackground: c.selection ?? `${cursor}55`,
    ...legiblePalette(ansi, own)
  }
}
