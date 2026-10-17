// Gates for the oracle's outputs. Run: node --test oracle.test.mjs
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { DATA_DIR, FIXTURE_DIR, EM_DASH } from './dump.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))

const jsonFiles = (dir) => readdirSync(dir).filter((f) => f.endsWith('.json')).map((f) => join(dir, f))
const readJson = (path) => JSON.parse(readFileSync(path, 'utf8'))

test('dump_counts_match_meta', () => {
  const meta = readJson(join(FIXTURE_DIR, 'meta.json'))
  const counted = jsonFiles(DATA_DIR).reduce((n, f) => n + readJson(f).length, 0)
  assert.equal(counted, meta.total)
  assert.ok(meta.total >= 250 && meta.total <= 600, `total ${meta.total} outside 250 to 600`)
  assert.equal(jsonFiles(DATA_DIR).length, 7)
})

test('no_em_dash_in_any_output', () => {
  for (const path of [...jsonFiles(DATA_DIR), ...jsonFiles(FIXTURE_DIR)]) {
    assert.ok(!readFileSync(path, 'utf8').includes(EM_DASH), `em-dash in ${path}`)
  }
  for (const f of readdirSync(join(HERE, 'src'))) {
    assert.ok(!readFileSync(join(HERE, 'src', f), 'utf8').includes(EM_DASH), `em-dash in src/${f}`)
  }
})

test('golden_has_more_than_2000_queries', () => {
  const golden = readJson(join(FIXTURE_DIR, 'golden_search.json'))
  assert.ok(golden.queries.length > 2000, `only ${golden.queries.length} queries`)
  assert.ok(
    readFileSync(join(FIXTURE_DIR, 'golden_search.json'), 'utf8').length <= 4 * 1048576,
    'golden_search.json is over 4 MB'
  )
})

test('golden_ids_are_catalogue_ids', () => {
  const ids = new Set(jsonFiles(DATA_DIR).flatMap((f) => readJson(f).map((e) => e.id)))
  const golden = readJson(join(FIXTURE_DIR, 'golden_search.json'))
  for (const row of golden.queries) {
    assert.equal(row.ids.length, row.scores.length)
    for (const id of row.ids) assert.ok(ids.has(id), `unknown id ${id} for "${row.q}"`)
  }
})

test('copy_check_passes', (t) => {
  const from = process.env.PT_HELP_SRC
  if (!from || !existsSync(from)) {
    t.skip('PT_HELP_SRC not set: the copies cannot be compared with Prism Terminal')
    return
  }
  execFileSync(process.execPath, [join(HERE, 'copy-sources.mjs'), '--from', from, '--check'], { stdio: 'pipe' })
})
