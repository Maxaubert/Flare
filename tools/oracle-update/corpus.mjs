// The seeded corpus the oracle feeds Prism Terminal's code (Task 2).
// Every number here is fixed: running gen.mjs twice writes the same bytes.

/** mulberry32: a small seeded PRNG. Returns a function giving floats in [0, 1). */
export function mulberry32(seed) {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const pick = (rng, xs) => xs[Math.floor(rng() * xs.length)]
const int = (rng, lo, hi) => lo + Math.floor(rng() * (hi - lo + 1))

/** Every sentence shape the Prism Terminal tests use, plus the hostile ones. */
export const FRAGMENTS = [
  "## What's Changed",
  '## New Contributors',
  '**Full Changelog**: https://github.com/a/b/compare/v1.0.0...v1.1.0',
  '- [ ] a task to do',
  '- [x] a finished task',
  '1. numbered one',
  '2) numbered two',
  '<!-- a comment, one line -->',
  '<!-- a comment\nover two lines -->',
  '<script>alert(1)</script>',
  '<scr<script>ipt>alert(2)</scr</script>ipt>',
  '[link](http://a.b/c(d))',
  '![alt text](x.png)',
  '**bold** words',
  '`inline code`',
  'snake_case_word in a sentence',
  'a lone x < y comparison',
  'control \u0000 \u0007 ‮ reversed ​ zero ﻿ bom',
  'crlf\r\nline',
  '\tindented with a tab',
  'emoji \u{1F600} and \u{1F680}',
  'fix(terminal): a prompt survives (#31)',
  'feat: a new thing',
  'chore(deps): bump a package',
  'Bump electron from 1 to 2',
  'Fix: the prompt',
  'Fixed-width font option',
  'a'.repeat(200),
  '* title with a surrogate ' + 'x'.repeat(137) + '\u{1F600} end',
  '# A heading',
  '---',
  'This release fixes the prompt, wrapped',
  '  continued under a bullet',
  '* Ordinary change by @someone in https://github.com/x/y/pull/7',
  '* Mention people by @handle in comments',
  '* Sort by name by @someone in https://x.test/pull/4',
  '* Links are painted in a theme colour by @Maxaubert in https://github.com/Maxaubert/Flare/pull/33',
]

/** A body of 1 to 14 fragments, joined with seeded separators. */
export function bodies(n, seed) {
  const rng = mulberry32(seed)
  const seps = ['\n', '\n\n', '\r\n', '\n', ' ']
  const out = []
  for (let i = 0; i < n; i += 1) {
    const count = int(rng, 1, 14)
    let body = ''
    for (let j = 0; j < count; j += 1) {
      if (j > 0) body += pick(rng, seps)
      body += pick(rng, FRAGMENTS)
    }
    out.push(body)
  }
  return out
}

/** Edge cases: the UTF-16 caps, the body cap, the controls, the surrogates. */
export const EDGE = [
  '* ' + 'a'.repeat(158) + '\u{1F600} tail',
  '* ' + 'a'.repeat(159) + '\u{1F600}',
  '* ' + 'a'.repeat(160),
  '* ' + 'a'.repeat(161),
  '* ' + '\u{1F600}'.repeat(100),
  '* ' + 'é'.repeat(300),
  '* ' + 'x'.repeat(20000),
  'a\n'.repeat(10001),
  'x'.repeat(20001),
  Array.from({ length: 25 }, (_, i) => `* entry ${i + 1}`).join('\n'),
  '* ' + '\u0007'.repeat(5) + 'a',
  '* a‮b⁦c⁩',
  '﻿* leading bom bullet',
  '* ' + ' '.repeat(200),
  '* [' + 'a'.repeat(300) + '](https://x)',
  '* ' + 'word '.repeat(60) + 'by @a in https://x.test/pull/12',
  '* a b',
]

/** Titles for the classify cases (single entries) and the group cases. */
export const TITLES = [
  'fix(terminal): a prompt survives (#31)',
  'feat: a new thing',
  'feature(core): something (#4)',
  'chore(deps): bump a package',
  'Bump electron from 1 to 2',
  'Fix: the prompt',
  'Fixes - the prompt',
  'Fixed-width font option',
  'Fix the prompt',
  'bugfix: a thing',
  'hotfix(ui): quick one (#9) (#10)',
  'perf: faster scrolling',
  'refactor(core)!: rename a thing',
  'style: tidy spacing',
  'ui: a button moves',
  'docs: a note',
  'test: one more check',
  'tests: more checks',
  'ci: a workflow',
  'build: a script',
  'revert: undo that',
  'deps: pin a version',
  'Phone: a cleaner tab drawer',
  'Explorer entries read "Open in Flare"',
  'Update dependencies for the week',
  'Mention people by @handle in comments',
  'A plain feature title',
  'Dictation: speak into the terminal',
  'Links are painted in a colour that follows the theme',
  'Sort by name',
  'Terminal core: prism-term-core v0.41.6',
  'Bump vite from 7.3.5 to 7.3.6',
  'xx: an unknown prefix',
  'fix(ci): the bump edits four lockfile lines (#25)',
  'Fixes the thing',
  'hotfix: patch',
  'fixed a crash (#77)',
  'Terminal core: prism-term-core v0.41.7 (#40)',
  'Feature: flagged',
  'chore: housekeeping (#1) (#2)',
]
