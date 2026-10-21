// COPY of PT/core/shared/help/types.ts, import specifiers rewritten, nothing else
/**
 * THE HELP PANEL'S CATALOGUE, its shape (#12).
 *
 * Owner, 2026-09-19: "make the shell more user friendly and the learning curve
 * less harsh: an easy to use panel where you can find shell commands", with
 * "metadata on each command so a natural-language search finds it ('how do I
 * find big files', not only the command's name)". And on 2026-09-20: "a pop up
 * with copy icons for easy copying, searchable, natural language".
 *
 * So an entry is written TASK FIRST: what someone wants to do, in the words
 * they would use, and only then the command that does it. The catalogue is
 * CURATED and ships with the app: it works offline, needs no model and no
 * network, and can be reviewed line by line. Nothing in it is ever run or typed
 * by the app (owner, 2026-09-19: picking a command does NOT insert it into the
 * shell). The panel shows, and copies on request.
 */

/** Which command language an entry is written in. 'any' is the same text in
 *  every shell (git, the agents' own CLIs, npm). 'windows' is the same text in
 *  PowerShell and Command Prompt and does NOT work in Bash (winget, wsl.exe,
 *  where.exe): a WSL tab never shows it (owner, 2026-10-13: a Bash list made
 *  "mostly" of things that "dont work in wsl" teaches nobody Linux). */
export type HelpShell = 'powershell' | 'cmd' | 'bash' | 'any' | 'windows'

export type HelpCategory =
  | 'files' // find, list, copy, move, delete, read, search inside
  | 'folders' // move around, make, size, tree
  | 'text' // search, filter, count, compare, replace
  | 'processes' // what is running, stop it, ports
  | 'network' // connectivity, downloads, ports, DNS
  | 'system' // disk, memory, environment, versions, services
  | 'packages' // winget, apt, npm, pip
  | 'git'
  | 'shell' // history, aliases, variables, the prompt, help itself
  | 'agents' // Claude Code and Codex
  | 'wsl' // Linux on Windows, run from the Windows side

export interface HelpEntry {
  /** Stable, unique, kebab-case, prefixed by its shell: 'ps-biggest-files'. */
  id: string
  shell: HelpShell
  category: HelpCategory
  /** What the person wants to do, as they would say it, sentence case, no full
   *  stop: "Find the biggest files in a folder". Under about 60 characters. */
  task: string
  /** One plain sentence on what the command does and when to reach for it. */
  summary: string
  /** THE command, copied as is. Placeholders are UPPER_SNAKE words the reader
   *  replaces (FOLDER, NAME, PORT): never angle brackets, which a shell reads
   *  as redirection. One line where the shell allows it. */
  command: string
  /**
   * Further ready-to-copy commands. Each is a ROW OF ITS OWN in the panel
   * (2026-09-20), so `label` is that row's NAME and has to stand alone: "Copy
   * the path to the clipboard", never "Copy it to the clipboard", which only
   * meant anything under its parent's title. Its `keywords` are that row's own
   * hidden words, read by the search and never shown; the entry's keywords
   * cover every row, so these are only what is true of THIS one.
   */
  variants?: ReadonlyArray<{
    label: string
    command: string
    keywords?: ReadonlyArray<string>
    /** Only in these shells: a row that is one shell's own spelling inside an
     *  entry the others share ("type FILE" is for cmd and PowerShell, not
     *  Bash). Absent: in every shell the entry is shown in. */
    shells?: ReadonlyArray<'powershell' | 'cmd' | 'bash'>
  }>
  /** What each placeholder stands for, when the command has any. */
  placeholders?: Readonly<Record<string, string>>
  /**
   * WORDS SOMEONE MIGHT TYPE WHO DOES NOT KNOW THE COMMAND: synonyms, the
   * everyday phrasing, the name of the same thing in another shell ("ls",
   * "dir", "grep"), the symptom ("port already in use"). Lower case. This is
   * what makes "how do I find big files" land, so be generous: 6 to 16.
   *
   * NONE OF THIS IS ON SCREEN (owner, 2026-09-20: "a lot of meta tags that are
   * not visible to the user but lets them search easier"). The row shows its
   * name and its command; the summary and these words are what the search
   * reads, which is why both keep being written even though neither is drawn.
   */
  keywords: ReadonlyArray<string>
  /** Set on anything that deletes, overwrites, kills or cannot be undone: one
   *  short sentence saying what is lost. The panel shows it as a warning. */
  danger?: string
}
