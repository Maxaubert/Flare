// Flare's own help entries, laid over Prism Terminal's catalogue. The copies in
// ./src stay byte-for-byte Prism Terminal's (copy-sources.mjs --check holds
// them); what Flare adds lives here, and dump.mjs and golden.mjs read the
// catalogue through this file, so the golden vectors are still Prism
// Terminal's own searchHelp over what Flare ships.
//
// Antigravity CLI (agy), the third agent (spec 2026-10-26-agy-and-agent-motion,
// Part 1 "Names and keys"): start `agy`, resume `agy -c` (most recent
// conversation in the folder, the resume Flare itself uses).

import { AGENTS_HELP } from './src/agents.ts'
import { ALL_HELP, HELP_CATEGORIES } from './src/index.ts'

/** Flare's entries for the agents file, after Prism Terminal's own. */
export const FLARE_AGENTS_HELP = [
  {
    id: 'agy-start',
    shell: 'any',
    category: 'agents',
    task: 'Start Antigravity in this folder',
    summary:
      "Opens an interactive session of Google's Antigravity CLI on the folder you are in. Move to your project folder first: that folder is what it works on.",
    command: 'agy',
    keywords: [
      'agy',
      'antigravity',
      'start antigravity',
      'open antigravity',
      'google',
      'gemini',
      'coding agent',
      'run the agent',
      'new session'
    ]
  },
  {
    id: 'agy-continue',
    shell: 'any',
    category: 'agents',
    task: 'Carry on the last Antigravity conversation',
    summary:
      'Reopens the most recent Antigravity conversation that was held in THIS folder, with its history.',
    command: 'agy -c',
    keywords: [
      'continue',
      'resume',
      'last conversation',
      'pick up where i left off',
      'previous session',
      'get my chat back',
      'agy -c',
      'antigravity'
    ]
  }
]

/** The agents file as Flare ships it. */
export const FLARE_AGENTS_FILE = [...AGENTS_HELP, ...FLARE_AGENTS_HELP]

/** The whole catalogue in file order, Flare's entries at the end of the agents file. */
export const FLARE_ALL_HELP = (() => {
  const at = ALL_HELP.indexOf(AGENTS_HELP[AGENTS_HELP.length - 1]) + 1
  return [...ALL_HELP.slice(0, at), ...FLARE_AGENTS_HELP, ...ALL_HELP.slice(at)]
})()

/** Category names Flare shows instead of Prism Terminal's. */
const FLARE_CATEGORY_NAMES = { agents: 'Claude Code, Codex and Antigravity' }

export const FLARE_HELP_CATEGORIES = HELP_CATEGORIES.map((c) => ({
  ...c,
  name: FLARE_CATEGORY_NAMES[c.id] ?? c.name
}))
