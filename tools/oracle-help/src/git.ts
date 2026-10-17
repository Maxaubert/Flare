// COPY of PT/core/shared/help/git.ts, import specifiers rewritten, nothing else
/**
 * THE HELP PANEL'S CATALOGUE: git (#12).
 *
 * Git is the same text in PowerShell, cmd and bash, so every entry here is
 * shell 'any'. That is a promise, and it shaped the wording: no `&&` (Windows
 * PowerShell 5.1 has no such operator), no `HEAD^` (cmd reads the caret as its
 * escape character, so `HEAD~1` is used throughout), and no `stash@{0}` or
 * `@{u}` (PowerShell reads `@{` as the start of a hashtable).
 *
 * HOW THESE WERE CHECKED (2026-09-20, git 2.55 for Windows): every command
 * and variant below was run in a throwaway repository with a throwaway bare
 * remote and its own GIT_CONFIG_GLOBAL, the destructive ones included, since
 * nothing in that folder mattered. The quoting-sensitive ones (-S "TEXT",
 * --grep "TEXT", main...HEAD, -m "MESSAGE") were run again under PowerShell
 * 7.6 and Windows PowerShell 5.1. `git merge --continue` and `git cherry-pick`
 * were not exercised to completion; they are long-standing git and are listed
 * on that strength.
 *
 * The newer verbs (`switch`, `restore`, git 2.23 and later) lead, because
 * they each do one thing; the `checkout` spelling is kept as a variant, since
 * that is what most tutorials and most agents still type.
 *
 * EVERY NAME STANDS ALONE (owner, 2026-09-20). The panel draws one row per
 * command, so a variant's `label` is a row name with nothing above it to lean
 * on: "The older spelling of the same thing" became "Switch branch with
 * checkout". Each variant also carries its own hidden `keywords`, never drawn,
 * for the words somebody would actually type to find that one row.
 */
import type { HelpEntry } from './types.ts'

export const GIT_HELP: readonly HelpEntry[] = [
  // ---- Looking before you touch anything ----
  {
    id: 'git-status',
    shell: 'any',
    category: 'git',
    task: 'See what has changed',
    summary:
      'Lists the files you have changed, which of them are staged for the next commit, and which branch you are on. Safe to run at any time, and the first thing to run when unsure.',
    command: 'git status',
    variants: [
      {
        label: 'Short form, one line per file',
        command: 'git status --short',
        keywords: ['porcelain', 'compact status', '--short', 'quick overview', 'terse', 'one line each']
      },
      {
        label: 'Short status, plus ahead or behind count',
        command: 'git status -sb',
        keywords: ['upstream', 'tracking', 'origin', 'how far behind', 'sb', 'branch summary']
      }
    ],
    keywords: [
      'what changed',
      'changed files',
      'modified',
      'untracked',
      'staged',
      'which branch am i on',
      'state of the repo',
      'anything to commit',
      'status',
      'where am i in git'
    ]
  },
  {
    id: 'git-diff',
    shell: 'any',
    category: 'git',
    task: 'See the exact lines you changed',
    summary:
      'Shows your uncommitted edits line by line: red lines were removed, green lines were added. Press q to leave the viewer.',
    command: 'git diff',
    variants: [
      {
        label: 'Diff the staged changes only',
        command: 'git diff --staged',
        keywords: ['cached', 'index', 'what will be committed', 'review before committing', 'about to commit']
      },
      {
        label: 'Summary of files and line counts',
        command: 'git diff --stat',
        keywords: ['--stat', 'diffstat', 'how many lines changed', 'insertions deletions', 'overview']
      }
    ],
    keywords: [
      'diff',
      'what did i change',
      'show changes',
      'line by line',
      'compare',
      'before commit',
      'staged changes',
      'cached',
      'review my edits',
      'how do i get out of the diff',
      'diff one file',
      'changes in this file'
    ]
  },
  {
    id: 'git-log',
    shell: 'any',
    category: 'git',
    task: 'See the history of commits',
    summary:
      'Lists recent commits, one line each, with the branches drawn as a graph. Press q to leave the viewer.',
    command: 'git log --oneline --graph --decorate -n 20',
    variants: [
      {
        label: 'Full commit messages, authors and dates',
        command: 'git log -n 10',
        keywords: ['verbose log', 'detailed history', 'timestamps', 'who and when', 'long form']
      },
      {
        label: 'Commits by one author',
        command: 'git log --oneline --author "NAME"',
        keywords: ['--author', 'filter by person', 'my commits', 'who committed what', 'by email']
      }
    ],
    placeholders: { NAME: "Part of the author's name or email" },
    keywords: [
      'history',
      'log',
      'past commits',
      'recent commits',
      'what happened',
      'timeline',
      'graph',
      'who committed',
      'commit list',
      'stuck in log press q'
    ]
  },
  {
    id: 'git-show',
    shell: 'any',
    category: 'git',
    task: 'See what one commit changed',
    summary:
      'Shows the message and the full diff of a single commit. Take the COMMIT id from git log; HEAD means the latest one.',
    command: 'git show COMMIT',
    variants: [
      {
        label: 'Show the latest commit',
        command: 'git show HEAD',
        keywords: ['head', 'last commit', 'most recent change', 'what did i just commit', 'top of the branch']
      },
      {
        label: 'List the files a commit touched',
        command: 'git show --stat COMMIT',
        keywords: ['--stat', 'which files changed', 'file list', 'overview of a commit', 'no diff']
      }
    ],
    placeholders: { COMMIT: 'A commit id from git log (the first 7 characters are enough)' },
    keywords: [
      'show commit',
      'what did this commit do',
      'inspect commit',
      'commit details',
      'commit diff',
      'hash',
      'sha',
      'last commit'
    ]
  },
  {
    id: 'git-file-history',
    shell: 'any',
    category: 'git',
    task: 'See the history of one file',
    summary: 'Lists only the commits that touched one file, newest first.',
    command: 'git log --oneline -- FILE',
    variants: [
      {
        label: 'File history with the diff of each commit',
        command: 'git log -p -- FILE',
        keywords: ['-p', 'patch', 'line changes over time', 'how this file evolved', 'full detail']
      },
      {
        label: 'Follow the file through renames',
        command: 'git log --oneline --follow -- FILE',
        keywords: ['--follow', 'moved file', 'renamed', 'history before the rename', 'was called something else']
      }
    ],
    placeholders: { FILE: 'The path of the file' },
    keywords: [
      'file history',
      'when did this file change',
      'changes to one file',
      'log for a file',
      'who touched this file',
      'renamed file history',
      'follow'
    ]
  },
  {
    id: 'git-blame',
    shell: 'any',
    category: 'git',
    task: 'Find out who changed a line, and when',
    summary:
      'Prints the file with the commit, author and date that last changed each line. Follow up with git show COMMIT to see why.',
    command: 'git blame FILE',
    variants: [
      {
        label: 'Blame only lines 10 to 20',
        command: 'git blame -L 10,20 FILE',
        keywords: ['line numbers', 'line range', 'annotate a section', 'part of a file', 'who wrote this function']
      }
    ],
    placeholders: { FILE: 'The path of the file' },
    keywords: [
      'blame',
      'who wrote this',
      'who changed this line',
      'author of a line',
      'annotate',
      'when was this changed',
      'last modified by',
      'praise'
    ]
  },
  {
    id: 'git-search-history',
    shell: 'any',
    category: 'git',
    task: 'Find the commit that added or removed text',
    summary:
      'Searches the whole history for commits where the number of times TEXT appears changed, which is how you find where a function or a setting came from or went.',
    command: 'git log --oneline -S "TEXT"',
    variants: [
      {
        label: 'Search commit messages for a word',
        command: 'git log --oneline --grep "TEXT"',
        keywords: ['subject line', 'find the commit that mentions', 'ticket number', 'by description', 'message search', 'search the log', 'message contains']
      }
    ],
    placeholders: { TEXT: 'The exact text to look for' },
    keywords: [
      'search history',
      'when was this added',
      'when was this deleted',
      'find in old commits',
      'pickaxe',
      'where did this code go',
      'grep history',
      'find commit'
    ]
  },
  {
    id: 'git-compare-main',
    shell: 'any',
    category: 'git',
    task: 'Compare your branch with main',
    summary:
      'Shows everything your branch has changed since it left main, which is what a pull request would contain. The three dots matter: two dots would also show what main gained meanwhile.',
    command: 'git diff main...HEAD',
    variants: [
      {
        label: 'Files changed against main, with counts',
        command: 'git diff --stat main...HEAD',
        keywords: ['--stat', 'diffstat', 'how big is the pr', 'file list', 'lines added removed']
      },
      {
        label: 'Commits on your branch but not main',
        command: 'git log main..HEAD --oneline',
        keywords: ['two dots', 'pr commits', 'what i have added', 'ahead of main', 'commit list for review']
      },
      {
        label: 'Compare with main including uncommitted edits',
        command: 'git diff main',
        keywords: ['two dot diff', 'working tree', 'unstaged too', 'right now', 'everything different']
      }
    ],
    keywords: [
      'compare branches',
      'diff with main',
      'diff against master',
      'what is in my branch',
      'pull request contents',
      'pr diff',
      'three dots',
      'ahead of main',
      'branch changes'
    ]
  },
  {
    id: 'git-unpushed',
    shell: 'any',
    category: 'git',
    task: 'See which commits you have not pushed yet',
    summary: 'Lists commits that exist on your machine and on no remote, so you know what a push would send.',
    command: 'git log --branches --not --remotes --oneline',
    keywords: [
      'unpushed',
      'how many to push',
      'not pushed',
      'ahead of origin',
      'local commits',
      'what will push send',
      'did i push',
      'outgoing'
    ]
  },

  // ---- The everyday loop: stage, commit, sync ----
  {
    id: 'git-add',
    shell: 'any',
    category: 'git',
    task: 'Stage files for the next commit',
    summary:
      'Staging is choosing what goes into the next commit. Stage one file, or everything; nothing is saved to history until you commit.',
    command: 'git add FILE',
    variants: [
      {
        label: 'Stage everything in this folder and below',
        command: 'git add .',
        keywords: ['add dot', 'add .', 'recursive', 'all my changes', 'whole directory']
      },
      {
        label: 'Stage every change, deletions included',
        command: 'git add -A',
        keywords: ['add -a', 'whole repository', 'removed files', 'add all', 'deleted files too']
      },
      {
        label: 'Stage changes piece by piece',
        command: 'git add -p',
        keywords: ['-p', 'interactive', 'hunk', 'patch mode', 'split my changes', 'partial staging']
      }
    ],
    placeholders: { FILE: 'The path of the file to stage' },
    keywords: [
      'stage',
      'add',
      'add files',
      'track a new file',
      'include in commit',
      'index',
      'untracked file',
      'add everything',
      'partial',
      'changes not staged for commit'
    ]
  },
  {
    id: 'git-unstage',
    shell: 'any',
    category: 'git',
    task: 'Unstage a file (keep the edits)',
    summary: 'Takes a file back out of the next commit. Your edits stay exactly as they are; only the staging is undone.',
    command: 'git restore --staged FILE',
    variants: [
      {
        label: 'Unstage everything',
        command: 'git restore --staged .',
        keywords: ['clear the index', 'undo git add .', 'take it all back out', 'nothing staged']
      },
      {
        label: 'Unstage a file with reset (older form)',
        command: 'git reset HEAD FILE',
        keywords: ['reset head', 'old syntax', 'pre 2.23', 'classic way', 'what tutorials use']
      }
    ],
    placeholders: { FILE: 'The path of the file to unstage' },
    keywords: [
      'unstage',
      'undo add',
      'remove from commit',
      'added the wrong file',
      'un-add',
      'reset file',
      'take out of staging',
      'git add undo'
    ]
  },
  {
    id: 'git-commit',
    shell: 'any',
    category: 'git',
    task: 'Commit your staged changes',
    summary:
      'Saves what you staged as one point in history, with a message saying what it does. It stays on your machine until you push.',
    command: 'git commit -m "MESSAGE"',
    variants: [
      {
        label: 'Stage tracked files and commit in one go',
        command: 'git commit -am "MESSAGE"',
        keywords: ['-am', 'skip git add', 'quick save', 'one step', 'commit everything tracked']
      },
      {
        label: 'Write a longer message in your editor',
        command: 'git commit',
        keywords: ['multi line message', 'commit body', 'vim opens', 'no -m flag', 'detailed description']
      }
    ],
    placeholders: { MESSAGE: 'A short sentence describing the change' },
    keywords: [
      'commit',
      'save changes',
      'save my work',
      'snapshot',
      'checkpoint',
      'commit message',
      'record changes',
      'nothing added to commit',
      'check in'
    ]
  },
  {
    id: 'git-amend',
    shell: 'any',
    category: 'git',
    task: 'Fix the last commit message',
    summary:
      'Replaces the message of the most recent commit. Do it before pushing: an amended commit is a new commit, so one that was already pushed needs a force push afterwards.',
    command: 'git commit --amend -m "NEW_MESSAGE"',
    variants: [
      {
        label: 'Add staged changes to the last commit',
        command: 'git commit --amend --no-edit',
        keywords: ['--no-edit', 'keep the message', 'forgot to include', 'missed a file', 'fold into previous']
      }
    ],
    placeholders: { NEW_MESSAGE: 'The corrected commit message' },
    keywords: [
      'amend',
      'typo in commit message',
      'change commit message',
      'edit last commit',
      'reword',
      'forgot a file',
      'add to last commit',
      'fix commit'
    ]
  },
  {
    id: 'git-undo-last-commit',
    shell: 'any',
    category: 'git',
    task: 'Undo the last commit but keep the work',
    summary:
      'Removes the most recent commit and leaves all of its changes staged, as if you had not committed yet. Meant for a commit you have NOT pushed; for a pushed one, use revert instead.',
    command: 'git reset --soft HEAD~1',
    variants: [
      {
        label: 'Undo the last commit, leave edits unstaged',
        command: 'git reset HEAD~1',
        keywords: ['mixed reset', 'back to the working tree', 'unstage as well', 'default reset', 'no --soft']
      },
      {
        label: 'Undo a pushed commit by reverting it',
        command: 'git revert HEAD',
        keywords: ['git revert head', 'safe on a shared branch', 'no force push', 'opposite commit', 'already on github']
      }
    ],
    keywords: [
      'undo commit',
      'uncommit',
      'committed too early',
      'committed to the wrong branch',
      'take back commit',
      'reset soft',
      'keep my changes',
      'go back one commit',
      'remove last commit'
    ]
  },
  {
    id: 'git-revert',
    shell: 'any',
    category: 'git',
    task: 'Undo a commit that was already pushed',
    summary:
      'Adds a NEW commit that does the opposite of an old one. History is not rewritten, so it is safe on a shared branch and needs no force push.',
    command: 'git revert COMMIT',
    variants: [
      {
        label: 'Reverse the latest commit, no editor',
        command: 'git revert --no-edit HEAD',
        keywords: ['--no-edit', 'revert head', 'automatic message', 'quick rollback', 'no prompt']
      }
    ],
    placeholders: { COMMIT: 'The id of the commit to reverse, from git log' },
    keywords: [
      'revert',
      'undo pushed commit',
      'back out a change',
      'reverse a commit',
      'rollback',
      'roll back',
      'safe undo',
      'undo on main'
    ]
  },
  {
    id: 'git-pull',
    shell: 'any',
    category: 'git',
    task: 'Update from the remote',
    summary:
      "Downloads the new commits for your branch and merges them into it. Commit or stash your own edits first if git refuses because they would be overwritten.",
    command: 'git pull',
    variants: [
      {
        label: 'Pull and rebase instead of merging',
        command: 'git pull --rebase',
        keywords: ['--rebase', 'linear history', 'no merge commit', 'replay my commits', 'tidy graph']
      },
      {
        label: 'Fetch only, change nothing in your files',
        command: 'git fetch',
        keywords: ['git fetch', 'download without merging', 'see what is new', 'update refs', 'safe check']
      },
      {
        label: 'Fetch and prune deleted remote branches',
        command: 'git fetch --prune',
        keywords: ['--prune', 'stale branches', 'gone branches', 'tidy origin', 'branch deleted on github']
      }
    ],
    keywords: [
      'pull',
      'update',
      'get latest',
      'sync',
      'download changes',
      'fetch',
      'refresh from github',
      'behind origin',
      'your local changes would be overwritten',
      'get new commits'
    ]
  },
  {
    id: 'git-push',
    shell: 'any',
    category: 'git',
    task: 'Push your commits to the remote',
    summary:
      'Uploads your committed work to the remote (GitHub, for most people). If it is rejected because the remote has commits you lack, pull first and push again.',
    command: 'git push',
    keywords: [
      'push',
      'upload',
      'send to github',
      'publish commits',
      'share my work',
      'rejected',
      'non-fast-forward',
      'failed to push some refs',
      'updates were rejected'
    ]
  },
  {
    id: 'git-push-new-branch',
    shell: 'any',
    category: 'git',
    task: 'Push a new branch for the first time',
    summary:
      'A branch you just made does not exist on the remote yet. This creates it there under the same name and links the two, so plain git push and git pull work from then on.',
    command: 'git push -u origin HEAD',
    variants: [
      {
        label: 'Push a new branch, naming it yourself',
        command: 'git push -u origin BRANCH',
        keywords: ['set upstream', '-u origin', 'explicit branch', 'different name on the remote', 'publish a branch']
      }
    ],
    placeholders: { BRANCH: 'The name of your branch' },
    keywords: [
      'push new branch',
      'no upstream branch',
      'set upstream',
      'has no upstream',
      'publish branch',
      'first push',
      'set-upstream',
      'track remote branch',
      'open a pull request'
    ]
  },

  // ---- Branches ----
  {
    id: 'git-new-branch',
    shell: 'any',
    category: 'git',
    task: 'Make a new branch and switch to it',
    summary:
      'Starts a branch from where you are now and moves you onto it. Uncommitted edits come along, so this is also how you move work you started on main by mistake.',
    command: 'git switch -c BRANCH',
    variants: [
      {
        label: 'Make a branch with checkout -b',
        command: 'git checkout -b BRANCH',
        keywords: ['checkout -b', 'older syntax', 'classic spelling', 'pre 2.23', 'what tutorials use', 'create and switch']
      },
      {
        label: 'Branch from the latest origin/main',
        command: 'git switch -c BRANCH --no-track origin/main',
        keywords: ['--no-track', 'start fresh', 'base on origin', 'not from here', 'up to date starting point']
      }
    ],
    placeholders: { BRANCH: 'The new branch name, no spaces (for example fix/login-button)' },
    keywords: [
      'new branch',
      'create branch',
      'make a branch',
      'start a feature',
      'checkout -b',
      'branch off',
      'working on main by mistake',
      'feature branch'
    ]
  },
  {
    id: 'git-switch-branch',
    shell: 'any',
    category: 'git',
    task: 'Switch to another branch',
    summary:
      'Moves you to an existing branch and updates your files to match it. Commit or stash first if git says your changes would be overwritten.',
    command: 'git switch BRANCH',
    variants: [
      {
        label: 'Back to the branch you were on before',
        command: 'git switch -',
        keywords: ['switch dash', 'previous branch', 'toggle between two', 'last one', 'jump back']
      },
      {
        label: 'Switch branch with checkout',
        command: 'git checkout BRANCH',
        keywords: ['git checkout', 'older syntax', 'classic spelling', 'pre 2.23', 'what tutorials use']
      }
    ],
    placeholders: { BRANCH: 'The name of the branch to move to' },
    keywords: [
      'switch branch',
      'change branch',
      'checkout',
      'go to main',
      'move to another branch',
      'previous branch',
      'go back to main',
      'please commit your changes or stash them'
    ]
  },
  {
    id: 'git-list-branches',
    shell: 'any',
    category: 'git',
    task: 'List branches and show the current one',
    summary: 'Lists your local branches with a star beside the current one.',
    command: 'git branch',
    variants: [
      {
        label: 'List remote branches too',
        command: 'git branch -a',
        keywords: ['-a', 'all branches', 'origin branches', 'everything on the server', 'what else exists']
      },
      {
        label: 'Print only the name of the current branch',
        command: 'git branch --show-current',
        keywords: ['--show-current', 'bare name for a script', 'which one am i on', 'no star', 'single line']
      }
    ],
    keywords: [
      'list branches',
      'which branch',
      'current branch',
      'what branch am i on',
      'show branches',
      'remote branches',
      'all branches',
      'branch name'
    ]
  },
  {
    id: 'git-rename-branch',
    shell: 'any',
    category: 'git',
    task: 'Rename the branch you are on',
    summary:
      'Changes the local name of the current branch. If it was already pushed, push it again under the new name and delete the old one on the remote.',
    command: 'git branch -m NEW_NAME',
    placeholders: { NEW_NAME: 'The new name for the current branch' },
    keywords: ['rename branch', 'change branch name', 'typo in branch name', 'branch -m', 'master to main', 'move branch']
  },
  {
    id: 'git-delete-branch',
    shell: 'any',
    category: 'git',
    task: 'Delete a branch',
    summary:
      'Removes a local branch you have finished with. The lower-case -d refuses when the branch holds work that is merged nowhere, which is the safety net; switch off the branch before deleting it.',
    command: 'git branch -d BRANCH',
    variants: [
      {
        label: 'Force-delete an unmerged branch',
        command: 'git branch -D BRANCH',
        keywords: ['capital d', 'branch -d', 'not fully merged', 'delete anyway', 'throw the work away']
      },
      {
        label: 'Delete the branch on the remote too',
        command: 'git push origin --delete BRANCH',
        keywords: ['push origin --delete', 'remove from github', 'server side', 'tidy origin', 'after a merged pr']
      }
    ],
    placeholders: { BRANCH: 'The name of the branch to delete' },
    keywords: [
      'delete branch',
      'remove branch',
      'clean up branches',
      'old branches',
      'not fully merged',
      'delete remote branch',
      'tidy branches',
      'cannot delete branch checked out'
    ],
    danger:
      'The -D and --delete forms remove a branch whether or not its commits exist anywhere else; unmerged work is then reachable only through git reflog, and only for a while.'
  },
  {
    id: 'git-merge',
    shell: 'any',
    category: 'git',
    task: 'Merge another branch into this one',
    summary:
      'Brings the commits of BRANCH into the branch you are on. Switch to the branch that should RECEIVE the work first (usually main), then merge the other one in.',
    command: 'git merge BRANCH',
    variants: [
      {
        label: 'Bring the latest main into your branch',
        command: 'git merge origin/main',
        keywords: ['merge origin/main', 'catch up', 'update from main', 'behind base branch', 'sync without rebasing']
      }
    ],
    placeholders: { BRANCH: 'The branch whose work you want to bring in' },
    keywords: [
      'merge',
      'combine branches',
      'bring in changes',
      'merge into main',
      'join branches',
      'fast-forward',
      'merge commit',
      'integrate'
    ]
  },
  {
    id: 'git-rebase-main',
    shell: 'any',
    category: 'git',
    task: 'Rebase your branch onto the latest main',
    summary:
      'Fetches main and replays your commits on top of it, so your branch reads as if it had started from today\'s main. If the branch was already pushed, the next push needs --force-with-lease.',
    command: 'git pull --rebase origin main',
    variants: [
      {
        label: 'Fetch origin before rebasing',
        command: 'git fetch origin',
        keywords: ['git fetch', 'download first', 'before rebasing', 'get the latest from origin', 'update the refs']
      },
      {
        label: 'Replay your commits onto origin/main',
        command: 'git rebase origin/main',
        keywords: ['git rebase', 'rebase my branch', 'rebase onto', 'move my commits', 'on top of main']
      }
    ],
    keywords: [
      'rebase',
      'rebase onto main',
      'update my branch',
      'branch is behind main',
      'out of date with base branch',
      'replay commits',
      'linear history',
      'catch up with main'
    ],
    danger:
      'A rebase rewrites the commits on your branch. Anyone else working on the same branch will conflict with it, so rebase only branches that are yours alone.'
  },
  {
    id: 'git-conflict',
    shell: 'any',
    category: 'git',
    task: 'Resolve a merge conflict',
    summary:
      'Lists the files in conflict. Open each one: between <<<<<<< and ======= is YOUR side, between ======= and >>>>>>> is the INCOMING side. Edit it to what it should be, delete all three marker lines, save, stage the file, then continue.',
    command: 'git diff --name-only --diff-filter=U',
    variants: [
      {
        label: 'Mark a conflicted file as resolved',
        command: 'git add FILE',
        keywords: ['git add', 'stage the fix', 'after editing the markers', 'done with this one', 'clear the conflict']
      },
      {
        label: 'Finish the merge after resolving',
        command: 'git merge --continue',
        keywords: ['--continue', 'complete it', 'commit the merge', 'carry on', 'all conflicts fixed']
      },
      {
        label: 'Finish the rebase after resolving',
        command: 'git rebase --continue',
        keywords: ['--continue', 'next commit', 'carry on rebasing', 'complete it', 'stopped at patch']
      },
      {
        label: 'Keep their side of a conflicted file',
        command: 'git checkout --theirs FILE',
        keywords: ['--ours', '--theirs', 'keep my version', 'keep their version', 'discard one side']
      }
    ],
    placeholders: { FILE: 'The path of a conflicted file' },
    keywords: [
      'conflict',
      'merge conflict',
      'conflict markers',
      'what do the arrows mean',
      'head and equals signs',
      'both modified',
      'unmerged paths',
      'automatic merge failed',
      'fix conflicts and then commit',
      'ours theirs',
      'resolve'
    ]
  },
  {
    id: 'git-abort',
    shell: 'any',
    category: 'git',
    task: 'Abort a merge or rebase and go back',
    summary:
      'Gives up on a merge that went wrong and puts everything back as it was before you started. Nothing is lost except the conflict resolutions you had made so far.',
    command: 'git merge --abort',
    variants: [
      {
        label: 'Abort a rebase in progress',
        command: 'git rebase --abort',
        keywords: ['--abort', 'cancel it', 'get out of the rebase', 'too many conflicts', 'start again']
      },
      {
        label: 'Abort a cherry-pick in progress',
        command: 'git cherry-pick --abort',
        keywords: ['--abort', 'cancel it', 'stop applying the commit', 'back out', 'undo the pick']
      }
    ],
    keywords: [
      'abort',
      'cancel merge',
      'cancel rebase',
      'stop the merge',
      'get out of rebase',
      'rebase in progress',
      'start over',
      'go back to before the merge',
      'too many conflicts'
    ]
  },
  {
    id: 'git-cherry-pick',
    shell: 'any',
    category: 'git',
    task: 'Copy one commit onto this branch',
    summary:
      'Applies the change from a single commit on another branch as a new commit here, without merging the rest of that branch.',
    command: 'git cherry-pick COMMIT',
    placeholders: { COMMIT: 'The id of the commit to copy, from git log on the other branch' },
    keywords: [
      'cherry-pick',
      'cherry pick',
      'copy a commit',
      'move commit to another branch',
      'one commit only',
      'apply a commit',
      'backport',
      'committed to the wrong branch'
    ]
  },
  {
    id: 'git-detached-head',
    shell: 'any',
    category: 'git',
    task: 'Get back from a "detached HEAD"',
    summary:
      'Detached HEAD means you are looking at an old commit rather than standing on a branch. Nothing is broken. If you made no commits there, just switch back; if you did, put them on a branch first or they will be left behind.',
    command: 'git switch -',
    variants: [
      {
        label: 'Switch to main from a detached HEAD',
        command: 'git switch main',
        keywords: ['git switch main', 'go to the trunk', 'named branch', 'leave the old commit', 'back to normal']
      },
      {
        label: 'Save detached commits on a new branch',
        command: 'git switch -c BRANCH',
        keywords: ['switch -c', 'rescue my work', 'do not lose the commits', 'name a branch', 'keep what i did']
      }
    ],
    placeholders: { BRANCH: 'A name for the branch that will hold those commits' },
    keywords: [
      'detached head',
      'head detached at',
      'you are in detached head state',
      'not on a branch',
      'not currently on any branch',
      'how do i get back',
      'checked out a commit',
      'lost my branch'
    ]
  },

  // ---- Putting work aside ----
  {
    id: 'git-stash',
    shell: 'any',
    category: 'git',
    task: 'Put your changes aside, and get them back',
    summary:
      'Stash shelves your uncommitted edits and gives you a clean folder, for when you must switch branch or pull in the middle of something. Pop brings the newest stash back.',
    command: 'git stash',
    variants: [
      {
        label: 'Bring the newest stash back',
        command: 'git stash pop',
        keywords: ['pop', 'unstash', 'restore my edits', 'get my work back', 'apply and drop']
      },
      {
        label: 'Stash untracked files too',
        command: 'git stash -u',
        keywords: ['-u', '--include-untracked', 'brand new files', 'not yet added', 'everything in the folder']
      },
      {
        label: 'List the stashes you have',
        command: 'git stash list',
        keywords: ['stash list', 'what did i shelve', 'see the shelf', 'how many', 'which one is which']
      },
      {
        label: 'Apply a stash and keep it',
        command: 'git stash apply',
        keywords: ['do not drop', 'reuse on another branch', 'leave it on the shelf', 'apply twice', 'no pop']
      }
    ],
    keywords: [
      'stash',
      'put aside',
      'shelve',
      'save for later',
      'temporarily remove changes',
      'clean working tree',
      'unstash',
      'stash pop',
      'where did my changes go',
      'switch branch without committing'
    ]
  },

  // ---- Starting and configuring ----
  {
    id: 'git-clone',
    shell: 'any',
    category: 'git',
    task: 'Download (clone) a repository',
    summary:
      'Copies a repository from GitHub or elsewhere, with its full history, into a new folder under the one you are in. Type a folder name after the address to choose what the new folder is called.',
    command: 'git clone URL',
    variants: [
      {
        label: 'Shallow clone, latest snapshot only',
        command: 'git clone --depth 1 URL',
        keywords: ['--depth 1', 'fast download', 'no history', 'huge repository', 'save bandwidth']
      }
    ],
    placeholders: {
      URL: 'The address from the green Code button, such as https://github.com/OWNER/REPO.git'
    },
    keywords: [
      'clone',
      'download repo',
      'get the code',
      'copy from github',
      'checkout a project',
      'shallow clone',
      'download a project',
      'repository url',
      'target directory',
      'clone into a folder'
    ]
  },
  {
    id: 'git-init',
    shell: 'any',
    category: 'git',
    task: 'Start tracking a folder with git',
    summary:
      'Turns the folder you are in into a new, empty repository. Nothing is uploaded anywhere; add and commit your files next.',
    command: 'git init',
    variants: [
      {
        label: 'Start with the branch called main',
        command: 'git init -b main',
        keywords: ['-b main', 'default branch name', 'not master', 'initial branch', 'rename at creation']
      }
    ],
    keywords: [
      'init',
      'new repository',
      'new repo',
      'start git',
      'create a repository',
      'not a git repository',
      'fatal not a git repository',
      'version control this folder'
    ]
  },
  {
    id: 'git-remote',
    shell: 'any',
    category: 'git',
    task: 'See or change where the repository pushes to',
    summary: 'Shows the remote addresses this repository downloads from and uploads to; origin is the usual name.',
    command: 'git remote -v',
    variants: [
      {
        label: 'Connect a new local repository to GitHub',
        command: 'git remote add origin URL',
        keywords: ['remote add origin', 'link it up', 'first push destination', 'attach a server', 'after git init']
      },
      {
        label: 'Point origin at a different address',
        command: 'git remote set-url origin URL',
        keywords: ['set-url', 'change the url', 'https to ssh', 'repository renamed', 'moved the repo']
      }
    ],
    placeholders: { URL: 'The address of the repository on the server' },
    keywords: [
      'remote',
      'origin',
      'where does it push',
      'github url',
      'change remote',
      'add remote',
      'repository moved',
      'no configured push destination',
      'does not appear to be a git repository'
    ]
  },
  {
    id: 'git-identity',
    shell: 'any',
    category: 'git',
    task: 'Set your name and email for commits',
    summary:
      'Git stamps every commit with a name and an email, and refuses to commit until they are set. Run both lines once per machine; use the email of your GitHub account so commits link to you.',
    command: 'git config --global user.name "YOUR_NAME"',
    variants: [
      {
        label: 'Set the email address for commits',
        command: 'git config --global user.email "EMAIL"',
        keywords: ['user.email', 'github email', 'author identity', 'wrong email on my commits', 'set my email']
      },
      {
        label: 'List your global git settings',
        command: 'git config --global --list',
        keywords: ['--list', 'what is configured', 'check my name', 'gitconfig', 'show the config']
      }
    ],
    placeholders: { YOUR_NAME: 'Your name as it should appear in history (spaces are fine inside the quotes)', EMAIL: 'Your email address' },
    keywords: [
      'name and email',
      'user.name',
      'user.email',
      'please tell me who you are',
      'author identity unknown',
      'configure git',
      'set up git',
      'git config',
      'wrong author'
    ]
  },
  {
    id: 'git-untrack-ignored',
    shell: 'any',
    category: 'git',
    task: 'Ignore a file that git is already tracking',
    summary:
      'A .gitignore entry only affects files git is not tracking yet. This stops tracking the file while leaving it on your disk; add it to .gitignore and commit, and git leaves it alone from then on.',
    command: 'git rm --cached FILE',
    variants: [
      {
        label: 'Stop tracking a whole folder',
        command: 'git rm -r --cached FOLDER',
        keywords: ['-r --cached', 'node_modules', 'recursive', 'directory', 'keep the files on disk']
      },
      {
        label: 'Find which gitignore rule catches a file',
        command: 'git check-ignore -v FILE',
        keywords: ['check-ignore', '-v', 'why is this ignored', 'not working', 'which pattern matched']
      }
    ],
    placeholders: { FILE: 'The path of the file to stop tracking', FOLDER: 'The path of the folder to stop tracking' },
    keywords: [
      'gitignore',
      'gitignore not working',
      'ignore file',
      'stop tracking',
      'untrack',
      'committed .env by mistake',
      'node_modules in git',
      'remove from git keep file',
      'rm cached'
    ],
    danger:
      'The file stays on YOUR disk, but the commit records it as deleted, so it disappears from the folder of everyone else who pulls. It also remains in the older history, so a committed secret must still be changed.'
  },
  {
    id: 'git-tag',
    shell: 'any',
    category: 'git',
    task: 'Tag a version',
    summary:
      'A tag is a permanent name for one commit, normally a release such as v1.2.0. Tags are not sent by a plain push; push them by name.',
    command: 'git tag -a TAG -m "MESSAGE"',
    variants: [
      {
        label: 'Push a tag to the remote',
        command: 'git push origin TAG',
        keywords: ['push origin tag', 'tags are not pushed by default', 'publish the release', 'upload', 'trigger a release']
      },
      {
        label: 'List the tags',
        command: 'git tag',
        keywords: ['git tag', 'see the versions', 'which releases exist', 'show them', 'what is released']
      },
      {
        label: 'Delete a local tag',
        command: 'git tag -d TAG',
        keywords: ['tag -d', 'remove a version', 'wrong number', 'undo a tag', 'retag']
      }
    ],
    placeholders: { TAG: 'The tag name, such as v1.2.0', MESSAGE: 'A short note, such as Release 1.2.0' },
    keywords: [
      'tag',
      'version',
      'release',
      'mark a release',
      'v1.0.0',
      'push tags',
      'list tags',
      'annotated tag',
      'label a commit'
    ]
  },

  // ---- Getting things back ----
  {
    id: 'git-restore-old-version',
    shell: 'any',
    category: 'git',
    task: 'Restore a file from an older commit',
    summary:
      'Overwrites one file in your folder with the version from an earlier commit, and leaves everything else alone. Commit the result to keep it.',
    command: 'git restore --source=COMMIT FILE',
    variants: [
      {
        label: 'Bring back a file you just deleted',
        command: 'git restore FILE',
        keywords: ['undelete', 'recover', 'i removed it by mistake', 'put the file back', 'not committed yet']
      }
    ],
    placeholders: { COMMIT: 'The commit id to take the file from (or HEAD~1 for one commit ago)', FILE: 'The path of the file' },
    keywords: [
      'old version of a file',
      'restore file',
      'recover deleted file',
      'get file from previous commit',
      'go back to earlier version',
      'undelete',
      'accidentally deleted',
      'checkout file from commit'
    ],
    danger: 'The current contents of FILE are replaced; uncommitted edits in it are lost.'
  },
  {
    id: 'git-reflog',
    shell: 'any',
    category: 'git',
    task: 'Find commits you thought you had lost',
    summary:
      'Lists everywhere HEAD has been lately, including commits no branch points at any more (after a bad reset, a rebase, a deleted branch). Find the id, then put a branch on it.',
    command: 'git reflog -n 30',
    variants: [
      {
        label: 'Put a branch on a lost commit',
        command: 'git branch BRANCH COMMIT',
        keywords: ['rescue', 'make it reachable', 'save the sha', 'recover my work', 'from the reflog']
      }
    ],
    placeholders: { BRANCH: 'A name for the rescue branch', COMMIT: 'The id from the reflog' },
    keywords: [
      'reflog',
      'lost commits',
      'recover commit',
      'undo reset hard',
      'undo rebase',
      'deleted branch by mistake',
      'get my work back',
      'where did my commits go',
      'rescue'
    ]
  },

  // ---- The ones that destroy work ----
  {
    id: 'git-discard-file',
    shell: 'any',
    category: 'git',
    task: 'Discard your changes to a file',
    summary:
      'Throws away the uncommitted edits in one file and puts back the last committed version. Run git diff FILE first to see what you are about to lose.',
    command: 'git restore FILE',
    variants: [
      {
        label: 'Discard edits in this folder and below',
        command: 'git restore .',
        keywords: ['restore dot', 'whole folder', 'throw it all away', 'back to the last commit', 'start again']
      },
      {
        label: 'Discard a file with checkout',
        command: 'git checkout -- FILE',
        keywords: ['checkout --', 'older syntax', 'classic spelling', 'pre 2.23', 'what tutorials use']
      }
    ],
    placeholders: { FILE: 'The path of the file to put back' },
    keywords: [
      'discard changes',
      'undo changes',
      'undo my edits',
      'revert file',
      'throw away changes',
      'reset a file',
      'go back to last commit',
      'start over on a file',
      'the agent broke my file'
    ],
    danger: 'Uncommitted edits were never saved anywhere, so git cannot bring them back. There is no undo.'
  },
  {
    id: 'git-force-push',
    shell: 'any',
    category: 'git',
    task: 'Force-push safely after a rebase or amend',
    summary:
      'After rewriting commits that were already pushed, a normal push is rejected. --force-with-lease overwrites the remote branch ONLY if nobody has pushed to it since you last fetched; plain --force checks nothing, so prefer this always.',
    command: 'git push --force-with-lease',
    keywords: [
      'force push',
      'force-with-lease',
      'push rejected after rebase',
      'push after amend',
      'overwrite remote',
      'push -f',
      'diverged',
      'your branch and origin have diverged',
      'non-fast-forward'
    ],
    danger:
      'The commits on the remote branch are replaced by yours. Never do this to main or to a branch other people work on.'
  },
  {
    id: 'git-reset-hard',
    shell: 'any',
    category: 'git',
    task: 'Throw away ALL uncommitted changes',
    summary:
      'Resets every tracked file to the last commit, staged or not. New untracked files are left alone. Run git status first, and consider git stash, which does the same tidy-up and keeps a way back.',
    command: 'git reset --hard HEAD',
    variants: [
      {
        label: 'Make the branch identical to origin/main',
        command: 'git reset --hard origin/main',
        keywords: ['drop my local commits', 'match github', 'wipe the branch', 'fetch first', 'like a fresh clone']
      }
    ],
    keywords: [
      'reset hard',
      'discard everything',
      'throw away all changes',
      'start over',
      'back to last commit',
      'wipe my changes',
      'match the remote',
      'make it like github',
      'nuke changes'
    ],
    danger:
      'Every uncommitted edit in every tracked file is gone for good. Commits dropped by the origin/main form can be found again through git reflog; edits that were never committed cannot.'
  },
  {
    id: 'git-clean',
    shell: 'any',
    category: 'git',
    task: 'Delete untracked files and folders',
    summary:
      'Removes everything git is not tracking: build leftovers, scratch files, new files never added. ALWAYS run the preview variant first; it lists what would go and deletes nothing.',
    command: 'git clean -fd',
    variants: [
      {
        label: 'PREVIEW: list what would be deleted',
        command: 'git clean -nfd',
        keywords: ['-n', 'dry run', 'safe check', 'nothing is removed', 'see first']
      },
      {
        label: 'Delete gitignored files too',
        command: 'git clean -fdx',
        keywords: ['-x', 'node_modules', '.env', 'build output', 'full wipe', 'fresh checkout']
      }
    ],
    keywords: [
      'clean',
      'remove untracked files',
      'delete untracked',
      'delete new files',
      'clean working directory',
      'remove build files',
      'get rid of junk files',
      'dry run',
      'fresh checkout'
    ],
    danger:
      'The files are deleted from disk straight away, not moved to the Recycle Bin, and git never had a copy. The -x form also takes ignored files such as .env.'
  }
]
