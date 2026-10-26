#!/usr/bin/env node
// Writes the catalogue as data for crates/flare-help: one JSON file per shell
// file (one entry per line, so diffs read well), meta.json, and blankless.json
// (what the row draws and copies, for every entry and variant).
// Run after copy-sources.mjs: node dump.mjs
// Flare's own entries (flare.mjs) are laid over Prism Terminal's catalogue.

import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { BASH_HELP } from './src/bash.ts'
import { CMD_HELP } from './src/cmd.ts'
import { GIT_HELP } from './src/git.ts'
import { PACKAGES_HELP } from './src/packages.ts'
import { POWERSHELL_HELP } from './src/powershell.ts'
import { WSL_HELP } from './src/wsl.ts'
import { HELP_ID_PREFIXES, blankless, isKeyPress } from './src/index.ts'
import { HELP_SHELLS } from './src/shells.ts'
import { FLARE_AGENTS_FILE, FLARE_ALL_HELP, FLARE_HELP_CATEGORIES } from './flare.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
export const DATA_DIR = join(HERE, '..', '..', 'crates', 'flare-help', 'data')
export const FIXTURE_DIR = join(HERE, '..', '..', 'crates', 'flare-help', 'tests', 'fixtures')

/** The one character the no-em-dash rule forbids, written as an escape. */
export const EM_DASH = String.fromCharCode(0x2014)

export const FILE_LISTS = [
  ['powershell', POWERSHELL_HELP],
  ['cmd', CMD_HELP],
  ['bash', BASH_HELP],
  ['git', GIT_HELP],
  ['agents', FLARE_AGENTS_FILE],
  ['packages', PACKAGES_HELP],
  ['wsl', WSL_HELP]
]

/** One JSON array, one entry per line. */
export function arrayText(list) {
  return `[\n${list.map((e) => JSON.stringify(e)).join(',\n')}\n]\n`
}

/** Every string an output file holds, for the no-em-dash check. */
export function allStrings(value, out = []) {
  if (typeof value === 'string') out.push(value)
  else if (Array.isArray(value)) for (const v of value) allStrings(v, out)
  else if (value && typeof value === 'object') for (const v of Object.values(value)) allStrings(v, out)
  return out
}

export function blanklessCases(entries) {
  const cases = []
  for (const entry of entries) {
    const names = Object.keys(entry.placeholders ?? {})
    const rows = [{ command: entry.command }, ...(entry.variants ?? []).map((v) => ({ command: v.command }))]
    for (const row of rows) {
      cases.push({
        command: row.command,
        names,
        out: blankless(row.command, names),
        key: isKeyPress(row.command)
      })
    }
  }
  return cases
}

function main() {
  mkdirSync(DATA_DIR, { recursive: true })
  let total = 0
  for (const [name, list] of FILE_LISTS) {
    total += list.length
    writeFileSync(join(DATA_DIR, `${name}.json`), arrayText(list))
    console.log(`${name}.json ${list.length} entries`)
  }
  if (total < 250 || total > 600) throw new Error(`entry total ${total} outside PT's own 250 to 600 gate`)
  if (total !== FLARE_ALL_HELP.length) throw new Error(`the catalogue has ${FLARE_ALL_HELP.length}, the files ${total}`)

  const meta = {
    total,
    categories: FLARE_HELP_CATEGORIES,
    shells: HELP_SHELLS,
    idPrefixes: HELP_ID_PREFIXES
  }
  mkdirSync(FIXTURE_DIR, { recursive: true })
  writeFileSync(join(FIXTURE_DIR, 'meta.json'), `${JSON.stringify(meta, null, 2)}\n`)

  const cases = blanklessCases(FLARE_ALL_HELP)
  writeFileSync(join(FIXTURE_DIR, 'blankless.json'), `${JSON.stringify(cases, null, 2)}\n`)

  const dashes = [
    ...FILE_LISTS.flatMap(([, list]) => allStrings(list)),
    ...allStrings(meta),
    ...allStrings(cases)
  ].filter((s) => s.includes(EM_DASH))
  if (dashes.length > 0) throw new Error(`em-dash in ${dashes.length} strings`)
  console.log(`meta.json, blankless.json (${cases.length} cases), total ${total}`)
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) main()
