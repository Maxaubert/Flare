// COPY of PT/core/shared/help/shells.ts, import specifiers rewritten, nothing else
import type { HelpShell } from './types.ts'

/**
 * WHICH SHELLS THE HELP PANEL KNOWS (#12), in a file of its own ON PURPOSE:
 * `index.ts` imports the whole catalogue, several hundred entries of text, and
 * a host wants to load that only when the popup is first opened. What a host
 * needs BEFORE then (which chip to preselect, the type of a stored choice)
 * lives here and imports nothing heavy.
 */

/** The shells somebody can be IN. 'any' is not one of them: it marks an entry
 *  that reads the same in all three, and is never a chip. */
export type HelpShellChoice = Exclude<HelpShell, 'any' | 'windows'>

/** Whether an entry written for `entryShell` is shown to someone in `shell`:
 *  its own, every 'any' one, and a 'windows' one outside Bash. The one rule
 *  the panel, the search and `helpFor` all read. */
export function shownIn(entryShell: HelpShell, shell: HelpShellChoice): boolean {
  return entryShell === shell || entryShell === 'any' || (entryShell === 'windows' && shell !== 'bash')
}

/** The variants of an entry that are shown in `shell`. */
export function variantsIn<V extends { shells?: ReadonlyArray<HelpShellChoice> }>(
  variants: ReadonlyArray<V> | undefined,
  shell: HelpShellChoice
): V[] {
  return (variants ?? []).filter((v) => !v.shells || v.shells.includes(shell))
}

export const HELP_SHELLS: ReadonlyArray<{ id: HelpShellChoice; name: string }> = [
  { id: 'powershell', name: 'PowerShell' },
  { id: 'cmd', name: 'Command Prompt' },
  { id: 'bash', name: 'Bash' }
]

/**
 * Which command language a DETECTED shell speaks, from its id
 * (`core/main/shells.ts`: 'pwsh', 'powershell', 'cmd', and 'wsl-<distro>' per
 * WSL distribution). An id nobody knows reads as PowerShell, which is what
 * `shellById` falls back to when a saved shell has gone.
 */
export function shellOfShellId(id: string | null | undefined): HelpShellChoice {
  const v = typeof id === 'string' ? id.toLowerCase() : ''
  if (v === 'cmd') return 'cmd'
  if (v.startsWith('wsl') || v.includes('bash')) return 'bash'
  return 'powershell'
}
