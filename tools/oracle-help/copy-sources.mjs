#!/usr/bin/env node
// Copies Prism Terminal's command-help sources into ./src so Node 24 can run
// them with its built-in type stripping. The ONLY edits: the first line is a
// header, and relative import specifiers get a '.ts' extension.
//
// Usage:
//   node copy-sources.mjs --from <PT/core/shared/help>   (writes ./src)
//   node copy-sources.mjs --from <dir> --check            (exit 1 on any difference)
// The source folder may also come from the PT_HELP_SRC environment variable.

import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

export const FILES = [
  'types',
  'shells',
  'search',
  'index',
  'powershell',
  'cmd',
  'bash',
  'git',
  'agents',
  'packages',
  'wsl'
]

const HERE = dirname(fileURLToPath(import.meta.url))
export const SRC_DIR = join(HERE, 'src')

// A relative import or re-export: from './name' becomes from './name.ts'.
const RELATIVE = /(\bfrom\s+')(\.\/[A-Za-z0-9_-]+)(')/g

/** The copy of one source file: a header line, then the source with its
 *  relative specifiers given the '.ts' extension, nothing else. */
export function transform(name, text) {
  const header = `// COPY of PT/core/shared/help/${name}.ts, import specifiers rewritten, nothing else\n`
  return header + text.replace(RELATIVE, '$1$2.ts$3')
}

function argValue(flag) {
  const i = process.argv.indexOf(flag)
  return i >= 0 ? process.argv[i + 1] : undefined
}

export function sourceDir() {
  const from = argValue('--from') ?? process.env.PT_HELP_SRC
  if (!from) throw new Error('Give the Prism Terminal help folder with --from <dir> or PT_HELP_SRC')
  return resolve(from)
}

function main() {
  const check = process.argv.includes('--check')
  const from = sourceDir()
  let differs = 0
  if (!check) mkdirSync(SRC_DIR, { recursive: true })
  for (const name of FILES) {
    const source = join(from, `${name}.ts`)
    if (!existsSync(source)) throw new Error(`missing ${source}`)
    const expected = transform(name, readFileSync(source, 'utf8'))
    const target = join(SRC_DIR, `${name}.ts`)
    if (check) {
      const have = existsSync(target) ? readFileSync(target, 'utf8') : null
      if (have !== expected) {
        differs += 1
        console.error(`differs: src/${name}.ts`)
      }
    } else {
      writeFileSync(target, expected)
      console.log(`copied src/${name}.ts`)
    }
  }
  if (check && differs > 0) process.exit(1)
  if (check) console.log('copy check passed')
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) main()
