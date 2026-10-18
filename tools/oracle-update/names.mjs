// Prism Terminal's own identifiers, read from its source by regex (Task 2).
// Writes the LEFT side only: Flare's names live in crates/flare-win/src/names.rs
// and are compared to these by crates/flare-win/tests/names.rs (Task 35).
// A value that cannot be found is an error, never a guess.

import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'

const SKIP_DIRS = new Set(['node_modules', 'dist', 'out', '.git', 'build'])

/** Every source file under `dirs` (not tests, not generated), as [relPath, text]. */
function sourceFiles(root, dirs) {
  const out = []
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      if (SKIP_DIRS.has(name)) continue
      const p = join(dir, name)
      if (statSync(p).isDirectory()) walk(p)
      else if (/\.(ts|tsx|mjs|cjs|js|cmd|json|yml|nsh)$/.test(name) && !/\.test\./.test(name)) {
        out.push([relative(root, p), readFileSync(p, 'utf8')])
      }
    }
  }
  for (const d of dirs) walk(join(root, d))
  return out
}

/** The first match of `re` in any file, its first capture group. Throws when none. */
function find(files, what, re) {
  for (const [path, text] of files) {
    const m = re.exec(text)
    if (m) return { value: m[1], path }
  }
  throw new Error(`names.mjs: ${what} not found (pattern ${re})`)
}

/** Reads every identifier of the left column of Appendix B. */
export function readPtNames(root) {
  const all = sourceFiles(root, ['src', 'core', 'build'])
  const yml = readFileSync(join(root, 'electron-builder.yml'), 'utf8')
  const ymlFiles = [['electron-builder.yml', yml]]
  const shell = all.filter(([p]) => /shellVerb\.ts$/.test(p))
  const index = all.filter(([p]) => /src[\\/]main[\\/]index\.ts$/.test(p))

  const appId = find(ymlFiles, 'appId', /^appId:\s*(\S+)/m).value
  const productName = find(ymlFiles, 'productName', /^productName:\s*(.+?)\s*$/m).value
  const artifact = find(ymlFiles, 'installer artifactName', /artifactName:\s*([A-Za-z]+)-Setup-\$\{arch\}/).value
  const exeName = find(ymlFiles, 'executableName', /^\s*executableName:\s*(\S+)/m)
    .value.replace(/\.exe$/, '') + '.exe'
  const profileFolder = find(index, 'userData folder', /setPath\('userData'[\s\S]*?'([A-Za-z]+)'\)\)/).value
  const dictationFolder = find(index, 'dictation folder', /'([A-Za-z]+Dictation)'/).value
  const aumid = find(index, 'AUMID', /setAppUserModelId\('([^']+)'\)/).value
  const verbLabel = find(shell, 'verb label', /VERB_LABEL\s*=\s*'([^']+)'/).value
  const verbKey = find(shell, 'verb key', /Directory\\\\shell\\\\([A-Za-z]+)/).value
  const plugin = find(all.filter(([p]) => /plugin\.json$/.test(p)), 'plugin name', /"name":\s*"([^"]+)"/).value
  const hookSignal = find(all.filter(([p]) => /hook\.cmd$/.test(p)), 'hook signal', /777;([a-z]+-agent);/).value
  const tabIdVar = find(all, 'tab id variable', /\b([A-Z]+_TAB_ID)\b/).value
  const resumeIdVar = find(all, 'resume id variable', /\b(PRISM_RESUME_ID)\b/).value
  const installRel = find(all, 'install folder', /'Programs',\s*'([A-Za-z]+)'/).value

  return {
    verbKey,
    verbLabel,
    appId,
    aumid,
    productName,
    exeName,
    installerPrefix: `${artifact}-Setup-x64-`,
    profileFolder,
    dictationFolder,
    pluginName: plugin,
    hookSignal,
    tabIdVar,
    resumeIdVar,
    installDir: `%LOCALAPPDATA%\\Programs\\${installRel}`,
    verbDirKey: `Directory\\shell\\${verbKey}`,
    verbBgKey: `Directory\\Background\\shell\\${verbKey}`,
  }
}
