// COPY of PT/core/shared/help/search.ts, import specifiers rewritten, nothing else
/**
 * THE HELP PANEL'S SEARCH (#12). Pure, offline and instant: no model, no
 * network, no dependency, because the panel searches on every keystroke and
 * has to work on a machine with no connection at all.
 *
 * Owner, 2026-09-19: "metadata on each command so a natural-language search
 * finds it ('how do I find big files', not only the command's name)". So the
 * query is read the way a person wrote it rather than matched as a substring:
 *
 *   1. the scaffolding of a question is dropped ("how do I", "the", "please"),
 *      but not the small words that carry meaning in a shell ("all", "not",
 *      "up", "out", "back", "who");
 *   2. words are folded to a stem, so "biggest files" meets "big file";
 *   3. a small synonym table covers the everyday vocabulary, so "erase a
 *      directory" meets "Delete a folder";
 *   4. a word still being typed matches by prefix, and a word of five letters
 *      or more that matches nothing at all is allowed ONE slip ("proccess"),
 *      each scored below the real thing.
 *
 * Someone who DOES know the command is served first: a query that is the
 * command's own name ("grep", "taskkill", "/compact") puts that entry on top.
 *
 * THE SCORE, highest tier first, so the order can be reasoned about:
 *   12000 / 10000  the query IS the command / its leading words
 *    6000          the same, for one of the entry's variants
 *    2000          the query is the start of the command (still being typed),
 *                  or names a later stage of its pipeline
 *    1000          EVERY meaningful word of the query matched this entry
 *    under 1000    the words themselves: per word, the best field it was found
 *                  in (task 10, keywords 6, summary 3, command 2, the rest 1),
 *                  scaled by how it matched and by how rare the word is, plus
 *                  up to 30 for a whole phrase ("where am i") found verbatim.
 * Ties break by the shorter task and then by id, so the order never depends on
 * where an entry sits in the catalogue.
 */
import type { HelpCategory, HelpEntry, HelpShell } from './types.ts'
import { shownIn } from './shells.ts'

export interface HelpSearchOptions {
  /** Keep the entries shown in this shell (`shownIn`); 'any' or 'windows'
   *  keeps the entries written for exactly that. */
  shell?: HelpShell
  category?: HelpCategory
  /** At most this many results. Anything that is not a positive number is no limit. */
  limit?: number
}

export interface HelpSearchHit {
  entry: HelpEntry
  score: number
}

// ---------------------------------------------------------------------------
// Words
// ---------------------------------------------------------------------------

/** A query is cut here before anything reads it: a pasted page is not a
 *  question, and the work below must stay bounded whatever arrives. */
const MAX_QUERY_CHARS = 400
/** No real word is longer, and a 5000-letter "word" must not be stemmed. */
const MAX_WORD_CHARS = 40
/** How many distinct meaningful words of a query are scored. */
const MAX_QUERY_WORDS = 24

const APOSTROPHES = /['’ʼ`]/g
const MARKS = /\p{M}+/gu
const NOT_WORD = /[^\p{L}\p{N}]+/u

/**
 * Lower-case words, nothing else: punctuation gone, accents folded, and an
 * apostrophe closed up rather than split on, so "what's" is "whats" (a stop
 * word) and not a stray "s". A command splits into its parts, which is what
 * lets "childitem" find Get-ChildItem.
 */
export function tokenise(text: string): string[] {
  if (typeof text !== 'string' || text === '') return []
  const flat = text.normalize('NFKD').replace(MARKS, '').replace(APOSTROPHES, '').toLowerCase()
  const out: string[] = []
  for (const w of flat.split(NOT_WORD)) {
    if (w !== '') out.push(w.length > MAX_WORD_CHARS ? w.slice(0, MAX_WORD_CHARS) : w)
  }
  return out
}

/**
 * The scaffolding of a question. Deliberately NOT here, because in a shell
 * they are the point: all, not, no, up, down, out, back, off, kill, who,
 * where, new, last, first, set, run, use. "get" IS here although it is
 * PowerShell's commonest verb: in "how do I get out of this folder" it matched
 * every Get- command in the catalogue, and someone who types Get-Process is
 * answered by the command's own name, which no stop word touches.
 */
const STOP_WORDS: ReadonlySet<string> = new Set(
  (
    'a an the this that these those there here it its ' +
    'i im ive id me my mine we our you your ' +
    'is are am be been was were do does did doing done ' +
    'can could would should will shall may might must ' +
    'how what whats which why when ' +
    'to of in on at by for with from into onto as about than then so and or but if ' +
    'please way ways want wants need needs like just some any thing things something someone ' +
    'tell know let lets help get one s t'
  ).split(' ')
)

/**
 * The words of a query that are worth scoring, each once, in the order they
 * were said. A query made of nothing but scaffolding ("how do i") keeps all
 * of its words: an answer to something beats an answer to nothing.
 */
export function meaningfulWords(query: string): string[] {
  const all = tokenise(typeof query === 'string' ? query.slice(0, MAX_QUERY_CHARS) : '')
  const kept = all.filter((w) => !STOP_WORDS.has(w))
  const seen = new Set<string>()
  const out: string[] = []
  for (const w of kept.length > 0 ? kept : all) {
    if (seen.has(w)) continue
    seen.add(w)
    out.push(w)
    if (out.length === MAX_QUERY_WORDS) break
  }
  return out
}

// ---------------------------------------------------------------------------
// Stems
// ---------------------------------------------------------------------------

/** Forms no suffix rule reaches. */
const IRREGULAR: ReadonlyMap<string, string> = new Map([
  ['using', 'use'],
  ['used', 'use'],
  ['uses', 'use'],
  ['going', 'go'],
  ['goes', 'go'],
  ['went', 'go'],
  ['ran', 'run'],
  ['made', 'make'],
  ['took', 'take'],
  ['taken', 'take'],
  ['wrote', 'write'],
  ['written', 'write'],
  ['found', 'find'],
  ['gone', 'go'],
  ['dirs', 'dir']
])

/**
 * -er and -est are folded ONLY onto these. English has far more nouns ending
 * that way than comparatives (folder, server, user, docker, test, latest), and
 * a search that turns "folder" into "fold" has broken the commonest word in
 * the catalogue. "new" is NOT one of them, on purpose: "newest files" asks
 * what changed last and "new file" asks how to make one, and folded together
 * the first question was answered with the second's command.
 */
const COMPARABLE: ReadonlySet<string> = new Set(
  'big large small old fast slow long short high low early heavy great few deep wide recent'.split(
    ' '
  )
)

const isVowel = (w: string, i: number): boolean => {
  const c = w[i]
  return c === 'a' || c === 'e' || c === 'i' || c === 'o' || c === 'u' || (c === 'y' && i > 0)
}
const hasVowel = (w: string): boolean => {
  for (let i = 0; i < w.length; i++) if (isVowel(w, i)) return true
  return false
}

/**
 * What is left once -ing / -ed / -er / -est has come off, put back into a
 * word: "stopp" is "stop", "delet" is "delete". Null when too little is left
 * to be a word at all ("ping" is not "p" plus -ing, "string" is not "str").
 */
function repair(root: string): string | null {
  if (root.length < 3 || !hasVowel(root)) return null
  const n = root.length
  // A doubled consonant was doubled BY the suffix (running, stopped, biggest),
  // except l, s and z, which double on their own (kill, miss, buzz). "add" is
  // left alone: what would remain is not a word.
  if (root[n - 1] === root[n - 2] && !isVowel(root, n - 1) && !'lsz'.includes(root[n - 1])) {
    return n > 3 ? root.slice(0, -1) : root
  }
  // The suffix ate a silent e. English cannot tell "deleted" from "edited"
  // by shape alone, so this is a guess, and a wrong one is harmless: see
  // `keyOf`, which is what the search actually compares.
  if (/(?:at|iz|bl|rg|ang|ur)$/.test(root)) return root + 'e'
  const cvc = !isVowel(root, n - 1) && isVowel(root, n - 2) && !isVowel(root, n - 3)
  if (cvc && !'wxy'.includes(root[n - 1]) && !/(?:en|er|on|or|el|al|it)$/.test(root))
    return root + 'e'
  return root
}

/**
 * A LIGHT stemmer: plurals, -ing, -ed, and -er / -est on a short list of
 * adjectives. Light on purpose. Words of three letters or fewer, and anything
 * that is not plain a-z, come back untouched, which is what keeps `ls`, `ps`,
 * `cls` and `sha256` themselves; "less", "status", "this" and "alias" survive
 * because -ss, -us, -is and -as are not plurals.
 */
export function stem(word: string): string {
  if (typeof word !== 'string') return ''
  if (word.length <= 3 || word.length > MAX_WORD_CHARS || !/^[a-z]+$/.test(word)) return word
  const irregular = IRREGULAR.get(word)
  if (irregular !== undefined) return irregular

  let w = word
  if (w.endsWith('sses')) w = w.slice(0, -2)
  else if (w.endsWith('ies') && w.length > 4) w = w.slice(0, -3) + 'y'
  else if (/(?:ch|sh|x)es$/.test(w)) w = w.slice(0, -2)
  else if (w.endsWith('s') && !/(?:ss|us|is|as)$/.test(w)) w = w.slice(0, -1)
  if (w.length <= 3) return w

  if (w.endsWith('ing')) return repair(w.slice(0, -3)) ?? w
  if (w.endsWith('ied') && w.length > 4) return w.slice(0, -3) + 'y'
  if (w.endsWith('ed') && !w.endsWith('eed')) return repair(w.slice(0, -2)) ?? w
  for (const [suffix, y] of [
    ['iest', true],
    ['ier', true],
    ['est', false],
    ['er', false]
  ] as const) {
    if (!w.endsWith(suffix)) continue
    const root = y ? w.slice(0, -suffix.length) + 'y' : repair(w.slice(0, -suffix.length))
    if (root !== null && COMPARABLE.has(root)) return root
    break
  }
  return w
}

/**
 * A short form and its long form are ONE word, not two that mean alike, so
 * they compare equal rather than as synonyms: "run as admin" has to meet the
 * keyword "run as administrator" as the same phrase. Kept to the handful a
 * shell's own vocabulary shortens; `dir` is not here, being a command too.
 */
const LONG_FORM: ReadonlyMap<string, string> = new Map([
  ['admin', 'administrator'],
  ['env', 'environment'],
  ['app', 'application'],
  ['repo', 'repository'],
  ['config', 'configuration']
])

const keyCache = new Map<string, string>()

/**
 * What two words are COMPARED by: the stem, less a final e on anything of
 * five letters or more. "deleted" and "edited" have the same shape and only
 * one of them lost an e, so `stem` sometimes restores one that was never
 * there ("develope"); ignoring that letter on both sides makes the guess
 * free. Four-letter words keep theirs, or "note" would meet "not".
 */
function keyOf(word: string): string {
  let k = keyCache.get(word)
  if (k === undefined) {
    k = stem(word)
    k = LONG_FORM.get(k) ?? k
    if (k.length >= 5 && k.endsWith('e')) k = k.slice(0, -1)
    // Bounded: query words are whatever anyone types, for as long as the app
    // runs. The catalogue's own few thousand words come straight back.
    if (keyCache.size > 20000) keyCache.clear()
    keyCache.set(word, k)
  }
  return k
}

// ---------------------------------------------------------------------------
// Synonyms
// ---------------------------------------------------------------------------

/**
 * The everyday vocabulary of this domain and nothing more. It is SMALL on
 * purpose: the catalogue's own keywords are where an entry says what else it
 * is called, and a big table here would make every query match everything.
 */
const SYNONYM_GROUPS: ReadonlyArray<ReadonlyArray<string>> = [
  ['big', 'large', 'huge', 'size', 'heavy'],
  ['delete', 'remove', 'erase', 'rm', 'del', 'trash'],
  ['folder', 'directory', 'dir'],
  ['find', 'search', 'locate', 'look'],
  ['show', 'list', 'see', 'display', 'view', 'print'],
  ['stop', 'kill', 'end', 'terminate', 'quit'],
  ['make', 'create', 'new'],
  ['rename', 'move', 'mv'],
  ['copy', 'duplicate', 'cp'],
  ['text', 'string', 'word'],
  ['process', 'task', 'program', 'app', 'application'],
  ['port', 'listening'],
  ['ip', 'address'],
  ['path', 'environment', 'env', 'variable'],
  ['undo', 'revert', 'discard', 'restore'],
  ['go', 'change', 'navigate', 'cd'],
  ['hidden', 'invisible', 'dotfile'],
  ['unzip', 'extract', 'decompress', 'unpack'],
  ['zip', 'compress', 'archive'],
  ['admin', 'administrator', 'elevated', 'sudo'],
  ['history', 'previous', 'last', 'recent'],
  ['newest', 'newer', 'latest'],
  ['clear', 'cls', 'clean', 'wipe'],
  ['install', 'setup'],
  ['download', 'fetch'],
  ['file', 'document'],
  ['frozen', 'hung', 'stuck', 'unresponsive']
]

/**
 * A word that reads as another ONE WAY, keyed by the word as typed. "running"
 * means a process, but its stem is "run", and "run a script" must not start
 * matching every entry about processes.
 */
const ONE_WAY: ReadonlyMap<string, string> = new Map([['running', 'process']])

const synonymsByKey: ReadonlyMap<string, ReadonlyArray<string>> = (() => {
  const m = new Map<string, string[]>()
  for (const group of SYNONYM_GROUPS) {
    for (const w of group) {
      const list = m.get(keyOf(w)) ?? []
      for (const other of group) if (other !== w && !list.includes(other)) list.push(other)
      m.set(keyOf(w), list)
    }
  }
  return m
})()

/**
 * The other everyday words for this one, as written in the table (never the
 * word itself). Empty for a word the table does not know.
 */
export function expand(word: string): string[] {
  if (typeof word !== 'string' || word === '') return []
  const w = word.toLowerCase()
  const via = ONE_WAY.get(w)
  if (via !== undefined) return [via, ...(synonymsByKey.get(keyOf(via)) ?? [])]
  const k = keyOf(w)
  return (synonymsByKey.get(k) ?? []).filter((s) => keyOf(s) !== k)
}

// ---------------------------------------------------------------------------
// One typo
// ---------------------------------------------------------------------------

/**
 * True when `a` is `b` with at most ONE slip: a letter wrong, missing, extra,
 * or two neighbours swapped ("direcotry"). Written out rather than as the
 * usual distance table because the bound is fixed at one, which makes it a
 * single pass with no allocation, and it runs against the whole vocabulary.
 */
export function withinOneEdit(a: string, b: string): boolean {
  const la = a.length
  const lb = b.length
  if (la - lb > 1 || lb - la > 1) return false
  let i = 0
  const n = la < lb ? la : lb
  while (i < n && a.charCodeAt(i) === b.charCodeAt(i)) i++
  if (i === la && i === lb) return true
  if (la === lb) {
    if (a.slice(i + 1) === b.slice(i + 1)) return true
    return i + 1 < la && a[i] === b[i + 1] && a[i + 1] === b[i] && a.slice(i + 2) === b.slice(i + 2)
  }
  return la > lb ? a.slice(i + 1) === b.slice(i) : a.slice(i) === b.slice(i + 1)
}

// ---------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------

const W_TASK = 10
const W_KEYWORDS = 6
const W_SUMMARY = 3
const W_COMMAND = 2
const W_REST = 1
// A VARIANT IS A ROW of its own in the panel (2026-09-20), so its name and its
// hidden words weigh a step under the entry's: enough that a row is found by
// what IT is called, not so much that a variant outranks an entry whose whole
// task is the question.
const W_VARIANT_NAME = 7
const W_VARIANT_KEY = 4.5
/** A second field holding the same word is a little more evidence, never
 *  enough to lift a lower field over a higher one. */
const ALSO = 0.25

/** How a query word can meet an entry's word, and what that is worth. A
 *  synonym sits just under 0.6 so the person's own word in an entry's
 *  keywords (6) beats a synonym of it in another entry's task (5.5). */
const M_EXACT = 1
const M_SYNONYM = 0.55
const M_PREFIX = 0.5
const M_FUZZY = 0.4

const BONUS_COMMAND_IS = 12000
const BONUS_COMMAND_LEADS = 10000
const BONUS_VARIANT = 6000
const BONUS_COMMAND_PARTIAL = 2000
const BONUS_ALL_WORDS = 1000
const BONUS_PHRASE_IS = 30
const BONUS_PHRASE_INSIDE = 15
const BONUS_PHRASE_AROUND = 12

interface Index {
  count: number
  /** key -> the entries holding it, and what holding it there is worth. */
  postings: Map<string, { at: number[]; worth: number[] }>
  vocabulary: string[]
  rarity: Map<string, number>
  /** Per entry: the command and its variants, lower-cased and single-spaced. */
  commands: string[]
  variants: string[][]
  /** Per entry: the first word of each LATER stage of the pipeline. */
  stages: string[][]
  /** Per entry: the task and each keyword as ' key key key '. */
  phrases: string[][]
}

const str = (v: unknown): string => (typeof v === 'string' ? v : '')
const flatCommand = (v: unknown): string => str(v).toLowerCase().replace(/\s+/g, ' ').trim()

function buildIndex(entries: readonly HelpEntry[]): Index {
  const count = entries.length
  const postings: Index['postings'] = new Map()
  const commands: string[] = []
  const variants: string[][] = []
  const stages: string[][] = []
  const phrases: string[][] = []

  for (let i = 0; i < count; i++) {
    // The types promise a well-formed entry; the panel must still open if a
    // catalogue file ever ships a broken one, so nothing here trusts a field.
    const entry = (entries[i] ?? {}) as Partial<HelpEntry>
    const fields = new Map<string, number[]>()
    const phrase: string[] = []
    const read = (text: unknown, worth: number, asPhrase: boolean): void => {
      const keys = tokenise(str(text)).map(keyOf)
      for (const k of keys) {
        const held = fields.get(k)
        if (held === undefined) fields.set(k, [worth])
        else if (!held.includes(worth)) held.push(worth)
      }
      if (asPhrase && keys.length > 0) phrase.push(` ${keys.join(' ')} `)
    }

    read(entry.task, W_TASK, true)
    const keywords = Array.isArray(entry.keywords) ? entry.keywords : []
    for (const k of keywords) read(k, W_KEYWORDS, true)
    read(entry.summary, W_SUMMARY, false)
    read(entry.command, W_COMMAND, false)
    // A VARIANT IS A ROW (2026-09-20), so its label is a name rather than a
    // caption and its own keywords are hidden words for that row. Both are
    // read a step below the entry's own, which keeps an entry whose TASK is
    // the answer ahead of one where only a variant matches - but well above
    // the old W_REST, where a row could not be found by its own name.
    const vs = Array.isArray(entry.variants) ? entry.variants : []
    for (const v of vs) {
      read(v?.label, W_VARIANT_NAME, true)
      for (const k of Array.isArray(v?.keywords) ? v.keywords : []) read(k, W_VARIANT_KEY, true)
      read(v?.command, W_REST, false)
    }
    if (entry.placeholders && typeof entry.placeholders === 'object') {
      for (const v of Object.values(entry.placeholders)) read(v, W_REST, false)
    }
    read(entry.danger, W_REST, false)

    for (const [k, held] of fields) {
      held.sort((a, b) => b - a)
      let worth = held[0]
      for (let j = 1; j < held.length; j++) worth += ALSO * held[j]
      let p = postings.get(k)
      if (p === undefined) postings.set(k, (p = { at: [], worth: [] }))
      p.at.push(i)
      p.worth.push(worth)
    }

    const command = flatCommand(entry.command)
    commands.push(command)
    variants.push(vs.map((v) => flatCommand(v?.command)).filter((c) => c !== ''))
    stages.push(
      command
        .split(/\|\||&&|[|;]/)
        .slice(1)
        .map(
          (s) =>
            s
              .trim()
              .replace(/^sudo /, '')
              .split(' ')[0]
        )
        .filter((s) => s !== '')
    )
    phrases.push(phrase)
  }

  // A word in every other entry ("file") says less than one in two ("port").
  // Between 1 and 2, so it tilts a ranking without overturning the fields.
  const rarity = new Map<string, number>()
  const scale = Math.log(count + 1) || 1
  for (const [k, p] of postings)
    rarity.set(k, 1 + Math.log((count + 1) / (p.at.length + 1)) / scale)

  return {
    count,
    postings,
    vocabulary: [...postings.keys()],
    rarity,
    commands,
    variants,
    stages,
    phrases
  }
}

/**
 * One index per catalogue ARRAY. The catalogue is a constant that ships with
 * the app, so every keystroke after the first finds its index already made;
 * a WeakMap, so a test's throwaway catalogue is not kept alive by it.
 */
const indexes = new WeakMap<readonly HelpEntry[], Index>()

function indexOf(entries: readonly HelpEntry[]): Index {
  let ix = indexes.get(entries)
  if (ix === undefined || ix.count !== entries.length) {
    ix = buildIndex(entries)
    indexes.set(entries, ix)
  }
  return ix
}

// ---------------------------------------------------------------------------
// The search
// ---------------------------------------------------------------------------

/** Every way this word of the query may meet a word of the catalogue. */
function candidates(word: string, ix: Index): Map<string, number> {
  const out = new Map<string, number>()
  const key = keyOf(word)
  out.set(key, M_EXACT)
  const others = expand(word)
  for (const s of others) {
    const k = keyOf(s)
    if (!out.has(k)) out.set(k, M_SYNONYM)
  }
  // A word the catalogue holds needs no guessing, and a number is a value for
  // a placeholder (the 3000 of "port 3000"), never a misspelling.
  if (ix.postings.has(key) || /^\d+$/.test(word)) return out

  // A guessed word brings its synonyms with it, or "direcotry" would find the
  // entries that say "directory" and miss the many more that say "folder".
  const guess = (v: string, how: number): void => {
    out.set(v, how)
    for (const s of synonymsByKey.get(v) ?? []) {
      const k = keyOf(s)
      if (!out.has(k)) out.set(k, how * M_SYNONYM)
    }
  }
  if (word.length >= 3) {
    for (const v of ix.vocabulary) {
      if (
        v.length > word.length &&
        (v.startsWith(word) || (key.length >= 3 && v.startsWith(key))) &&
        !out.has(v)
      ) {
        guess(v, M_PREFIX)
      }
    }
  }
  // ONE typo, only in a word long enough for one slip to leave it recognisable,
  // and only when nothing else explained the word. The first letter has to be
  // right: people rarely miss it, and without that rule "mount" meets "count".
  if (out.size === 1 && others.length === 0 && word.length >= 5) {
    for (const v of ix.vocabulary) {
      if (
        !out.has(v) &&
        v.length >= 4 &&
        v.charCodeAt(0) === word.charCodeAt(0) &&
        (withinOneEdit(word, v) || withinOneEdit(key, v))
      ) {
        guess(v, M_FUZZY)
      }
    }
  }
  return out
}

function commandBonus(q: string, i: number, ix: Index): number {
  if (q === '' || q.length > 200) return 0
  const command = ix.commands[i]
  if (command === q) return BONUS_COMMAND_IS
  const lead = q + ' '
  if (command.startsWith(lead)) return BONUS_COMMAND_LEADS
  const vs = ix.variants[i]
  for (const v of vs) if (v === q || v.startsWith(lead)) return BONUS_VARIANT
  if (ix.stages[i].includes(q)) return BONUS_COMMAND_PARTIAL
  // Still being typed: "/comp", "get-ch". One letter is too little to mean a
  // command, except a slash, which means nothing else.
  if (q.length >= 2 || q === '/') {
    if (command.startsWith(q)) return BONUS_COMMAND_PARTIAL
    for (const v of vs) if (v.startsWith(q)) return BONUS_COMMAND_PARTIAL
  }
  return 0
}

function phraseBonus(padded: string, wordCount: number, phrases: readonly string[]): number {
  let best = 0
  for (const p of phrases) {
    if (p === padded) return BONUS_PHRASE_IS
    // A one-word keyword inside a sentence is just a word, already scored.
    if (best < BONUS_PHRASE_INSIDE && p.indexOf(' ', 1) < p.length - 1 && padded.includes(p))
      best = BONUS_PHRASE_INSIDE
    else if (best < BONUS_PHRASE_AROUND && wordCount >= 2 && p.includes(padded))
      best = BONUS_PHRASE_AROUND
  }
  return best
}

/**
 * Search the catalogue. An EMPTY query is not a search: it answers with the
 * entries of that shell in catalogue order, score 0, so the panel can browse
 * by category. A query that matches nothing answers with nothing. Never
 * throws, whatever it is handed.
 */
export function searchHelp(
  entries: readonly HelpEntry[],
  query: string,
  opts?: HelpSearchOptions
): HelpSearchHit[] {
  if (!Array.isArray(entries) || entries.length === 0) return []
  const shell = opts?.shell
  const category = opts?.category
  const limit =
    typeof opts?.limit === 'number' && opts.limit >= 1 ? Math.floor(opts.limit) : Infinity
  const wanted = (entry: HelpEntry | undefined | null): entry is HelpEntry =>
    entry != null &&
    typeof entry === 'object' &&
    (shell === undefined ||
      (shell === 'any' || shell === 'windows' ? entry.shell === shell : shownIn(entry.shell, shell))) &&
    (category === undefined || entry.category === category)

  const text = typeof query === 'string' ? query.slice(0, MAX_QUERY_CHARS) : ''
  if (text.trim() === '') {
    const all: HelpSearchHit[] = []
    for (const entry of entries) {
      if (wanted(entry)) all.push({ entry, score: 0 })
      if (all.length >= limit) break
    }
    return all
  }

  const ix = indexOf(entries)
  const n = ix.count
  const words = meaningfulWords(text)
  const sum = new Float64Array(n)
  const matched = new Uint8Array(n)
  const best = new Float64Array(n)
  // A number is never REQUIRED: "port 3000" is fully answered by the entry
  // about ports, whose command says PORT where the 3000 will go.
  let required = 0

  for (const word of words) {
    const counts = !/^\d+$/.test(word)
    if (counts) required++
    best.fill(0)
    for (const [key, how] of candidates(word, ix)) {
      const p = ix.postings.get(key)
      if (p === undefined) continue
      const scale = how * (ix.rarity.get(key) ?? 1)
      for (let j = 0; j < p.at.length; j++) {
        const v = p.worth[j] * scale
        if (v > best[p.at[j]]) best[p.at[j]] = v
      }
    }
    for (let i = 0; i < n; i++) {
      if (best[i] === 0) continue
      sum[i] += best[i]
      if (counts) matched[i]++
    }
  }

  const q = flatCommand(text)
  const spoken = tokenise(text)
    .slice(0, MAX_QUERY_WORDS * 2)
    .map(keyOf)
  const padded = ` ${spoken.join(' ')} `

  const hits: HelpSearchHit[] = []
  for (let i = 0; i < n; i++) {
    const entry = entries[i]
    if (!wanted(entry)) continue
    const named = commandBonus(q, i, ix)
    const phrase = spoken.length > 0 ? phraseBonus(padded, spoken.length, ix.phrases[i]) : 0
    if (sum[i] === 0 && named === 0 && phrase === 0) continue
    const everyWord = required > 0 && matched[i] === required ? BONUS_ALL_WORDS : 0
    hits.push({ entry, score: Math.round((sum[i] + phrase + everyWord + named) * 100) / 100 })
  }

  hits.sort(
    (a, b) =>
      b.score - a.score ||
      str(a.entry.task).length - str(b.entry.task).length ||
      (str(a.entry.id) < str(b.entry.id) ? -1 : str(a.entry.id) > str(b.entry.id) ? 1 : 0)
  )
  return hits.length > limit ? hits.slice(0, limit) : hits
}
