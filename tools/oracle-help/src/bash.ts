// COPY of PT/core/shared/help/bash.ts, import specifiers rewritten, nothing else
/**
 * THE HELP CATALOGUE, bash: WSL, Linux and Git Bash (#12).
 *
 * Written for a Windows user who has landed in Ubuntu under WSL, usually to run
 * an agent there: the everyday file and folder work, finding text, processes
 * and ports, apt, the environment and ~/.bashrc, and the WSL seams a Windows
 * user trips on (the drives under /mnt/c, explorer.exe, line endings, chmod +x).
 * What is run from the WINDOWS side (wsl --shutdown, wsl -l -v) lives with the
 * PowerShell and cmd entries, not here. The keywords carry the Windows names
 * ("dir", "findstr", "tasklist") because that is what this reader knows.
 *
 * HOW THESE WERE CHECKED (2026-09-20): this machine has WSL but no distribution
 * installed, so the commands were RUN in Git Bash (GNU coreutils, the same
 * ls/cp/find/grep/sed/tar/du/df) inside a scratch folder made for the purpose:
 * every file, folder, text, redirection, chaining, environment, PATH, alias,
 * jobs and kill entry, plus curl, ssh -V, file and the CRLF fix. NOT RUN, since
 * Git Bash does not ship them, and written from the tools' documented
 * behaviour on Ubuntu: apt, sudo, man, lsof, ss, ip, pgrep and pkill, top,
 * free, lsb_release, getent, wget, zip, wslpath, /mnt/c and the .exe interop.
 * Where a tool is not in a stock Ubuntu image (unzip, zip, tree, dos2unix) the
 * summary says so and gives the apt line.
 */

import type { HelpEntry } from './types.ts'

export const BASH_HELP: readonly HelpEntry[] = [
  // ---------------------------------------------------------------- folders
  {
    id: 'sh-where-am-i',
    shell: 'bash',
    category: 'folders',
    task: 'See which folder I am in',
    summary: 'Prints the full path of the current folder ("print working directory").',
    command: 'pwd',
    keywords: [
      'where am i',
      'current folder',
      'current directory',
      'working directory',
      'pwd',
      'get-location',
      'cd with no arguments',
      'which folder'
    ]
  },
  {
    id: 'sh-change-folder',
    shell: 'bash',
    category: 'folders',
    task: 'Go to another folder',
    summary:
      'Moves into a folder. Names are case-sensitive here (Documents is not documents), and the slashes lean forward.',
    command: 'cd FOLDER',
    variants: [
      {
        label: 'Go to your home folder',
        command: 'cd ~',
        keywords: ['cd ~', 'tilde', 'home directory', 'user profile', 'my files', 'default folder']
      },
      {
        label: 'Go back to the previous folder',
        command: 'cd -',
        keywords: ['cd -', 'last directory', 'toggle folder', 'where i was', 'switch back']
      },
      {
        label: 'Go to a folder whose name has spaces',
        command: 'cd "/mnt/c/Program Files"',
        keywords: [
          'spaces in path',
          'quoting',
          'my documents',
          'program files',
          'no such file or directory'
        ]
      }
    ],
    placeholders: { FOLDER: 'The folder to go to, for example ~/projects/site' },
    keywords: [
      'cd',
      'change directory',
      'change folder',
      'go to folder',
      'navigate',
      'set-location',
      'home folder',
      'tilde',
      'no such file or directory',
      'previous folder'
    ]
  },
  {
    id: 'sh-go-up',
    shell: 'bash',
    category: 'folders',
    task: 'Go back up a folder',
    summary:
      'Two dots mean "the folder that holds this one". The space after cd is required here, unlike in Command Prompt.',
    command: 'cd ..',
    variants: [
      {
        label: 'Go up two folders',
        command: 'cd ../..',
        keywords: ['cd ../..', 'two levels', 'grandparent', 'up twice', 'back two']
      }
    ],
    keywords: [
      'go back',
      'back a folder',
      'parent folder',
      'up one level',
      'cd ..',
      'cd..',
      'dot dot',
      'leave folder',
      'command not found cd..'
    ]
  },
  {
    id: 'sh-windows-drives',
    shell: 'bash',
    category: 'folders',
    task: 'Reach my Windows files from WSL',
    summary:
      'WSL mounts each Windows drive under /mnt by its letter, so C:\\Users is /mnt/c/Users. It works, but it is slow: keep projects you build in WSL inside the Linux home folder instead.',
    command: 'cd /mnt/c/Users/NAME',
    variants: [
      {
        label: 'See which drives are mounted',
        command: 'ls /mnt',
        keywords: ['ls /mnt', 'c drive', 'd drive', 'mount points', 'available letters']
      },
      {
        label: 'Convert a Windows path to Linux',
        command: "wslpath 'C:\\Users\\NAME\\Documents'",
        keywords: ['wslpath', 'translate path', 'mnt c', 'path conversion', 'backslash']
      },
      {
        label: 'Show this folder as a Windows path',
        command: 'wslpath -w .',
        keywords: ['wslpath -w', 'unc', 'wsl.localhost', 'paste into explorer', 'windows form']
      }
    ],
    placeholders: { NAME: 'Your Windows user name' },
    keywords: [
      'mnt c',
      '/mnt/c',
      'c drive',
      'windows files',
      'windows drive',
      'access windows from wsl',
      'wslpath',
      'convert path',
      'where are my documents',
      'desktop folder',
      'downloads'
    ]
  },
  {
    id: 'sh-open-explorer',
    shell: 'bash',
    category: 'folders',
    task: 'Open this folder in File Explorer',
    summary:
      'WSL can start Windows programs by their .exe name. The dot means "here", so Explorer opens on the current Linux folder (under \\\\wsl.localhost).',
    command: 'explorer.exe .',
    variants: [
      {
        label: 'Open this folder in VS Code',
        command: 'code .',
        keywords: ['code .', 'vscode', 'visual studio code', 'editor', 'open project']
      },
      {
        label: 'Open a file in Notepad',
        command: 'notepad.exe FILE',
        keywords: ['notepad.exe', 'windows program', 'interop', 'edit in windows', 'run an exe']
      }
    ],
    placeholders: { FILE: 'A file in the current folder' },
    keywords: [
      'explorer',
      'file explorer',
      'open folder',
      'open in explorer',
      'show in explorer',
      'start .',
      'xdg-open',
      'see linux files in windows',
      'wsl$',
      'vs code',
      'run windows program'
    ]
  },
  {
    id: 'sh-make-folder',
    shell: 'bash',
    category: 'folders',
    task: 'Make a new folder',
    summary:
      '-p makes any missing folders on the way, and does not complain if the folder is already there.',
    command: 'mkdir -p NAME',
    variants: [
      {
        label: 'Make nested folders in one go',
        command: 'mkdir -p PARENT/CHILD/GRANDCHILD',
        keywords: ['mkdir -p', 'parent folders', 'create a tree', 'deep path', 'several levels']
      }
    ],
    placeholders: {
      NAME: 'The new folder',
      PARENT: 'The outer folder',
      CHILD: 'A folder inside it',
      GRANDCHILD: 'A folder inside that'
    },
    keywords: [
      'mkdir',
      'md',
      'create folder',
      'new folder',
      'make directory',
      'new directory',
      'new-item',
      'nested folders'
    ]
  },
  {
    id: 'sh-delete-folder',
    shell: 'bash',
    category: 'folders',
    task: 'Delete a folder and everything in it',
    summary:
      '-r means "and everything inside". Read the path twice before pressing Enter, and never run it on a path built from a variable that might be empty.',
    command: 'rm -r FOLDER',
    variants: [
      {
        label: 'Remove an empty folder',
        command: 'rmdir FOLDER',
        keywords: ['rmdir', 'directory not empty', 'safe delete', 'no recursion', 'only if empty']
      },
      {
        label: 'Force delete with no questions asked',
        command: 'rm -rf FOLDER',
        keywords: ['rm -rf', 'write protected', 'read only files', 'ignore errors', 'no prompt']
      }
    ],
    placeholders: { FOLDER: 'The folder to remove' },
    keywords: [
      'rm -rf',
      'rmdir',
      'remove folder',
      'delete directory',
      'rd /s',
      'remove-item -recurse',
      'directory not empty',
      'is a directory',
      'delete node_modules'
    ],
    danger:
      'There is no Recycle Bin in Linux. The folder and everything inside it is gone for good.'
  },
  {
    id: 'sh-folder-size',
    shell: 'bash',
    category: 'folders',
    task: 'See how big a folder is',
    summary:
      '-s gives one total instead of a line per subfolder, and -h prints it in K, M and G rather than blocks.',
    command: 'du -sh FOLDER',
    variants: [
      {
        label: 'Sort everything here by size, smallest first',
        command: 'du -sh * | sort -h',
        keywords: ['du -sh *', 'sort -h', 'compare folders', 'rank by size', 'what is hogging space']
      },
      {
        label: 'Size of each subfolder, one level deep',
        command: 'du -h --max-depth=1 .',
        keywords: ['max-depth', 'depth 1', 'per folder total', 'top level only', 'du -h']
      }
    ],
    placeholders: { FOLDER: 'The folder to measure, or . for the current one' },
    keywords: [
      'du',
      'du -sh',
      'folder size',
      'directory size',
      'how big',
      'disk usage',
      'space used',
      'what is taking up space',
      'total size'
    ]
  },
  {
    id: 'sh-tree',
    shell: 'bash',
    category: 'folders',
    task: 'Show the folder structure',
    summary:
      'Lists the folders two levels down using find, which is always there. The prettier tree command is not installed on a stock Ubuntu: sudo apt install tree.',
    command: 'find . -maxdepth 2 -type d',
    variants: [
      {
        label: 'Show the structure with tree',
        command: 'tree -L 2',
        keywords: ['tree -l 2', 'apt install tree', 'ascii tree', 'depth limit', 'tree not found']
      }
    ],
    keywords: [
      'tree',
      'folder structure',
      'directory structure',
      'project layout',
      'list subfolders',
      'show hierarchy',
      'tree command not found'
    ]
  },

  // ------------------------------------------------------------------ files
  {
    id: 'sh-list-files',
    shell: 'bash',
    category: 'files',
    task: 'List the files in this folder',
    summary:
      '-l shows sizes, dates and permissions, -a includes hidden files, and -h prints sizes in K, M and G.',
    command: 'ls -lah',
    variants: [
      {
        label: 'List file names only',
        command: 'ls',
        keywords: ['ls', 'bare list', 'no details', 'plain listing', 'just the names']
      },
      {
        label: 'List files newest first',
        command: 'ls -lt',
        keywords: ['ls -lt', 'sort by date', 'most recent', 'last modified', 'recently changed']
      },
      {
        label: 'List files biggest first',
        command: 'ls -lhS',
        keywords: ['ls -lhs', 'sort by size', 'size order', 'space hogs', 'which file is huge']
      }
    ],
    keywords: [
      'ls',
      'dir',
      'list files',
      'show files',
      'see files',
      'files here',
      'what files are here',
      'whats in this folder',
      'what is in this folder',
      'list everything',
      'll',
      'sort by date',
      'sort by size',
      'newest files'
    ]
  },
  {
    id: 'sh-hidden-files',
    shell: 'bash',
    category: 'files',
    task: 'Show hidden files and dotfiles',
    summary:
      'In Linux a file is hidden simply because its name starts with a dot (.env, .git, .bashrc). -a lists those as well.',
    command: 'ls -a',
    keywords: [
      'hidden files',
      'show hidden',
      'dotfiles',
      'dot files',
      'dir /a',
      'get-childitem -force',
      '.env missing',
      'where is .bashrc',
      'invisible files'
    ]
  },
  {
    id: 'sh-read-file',
    shell: 'bash',
    category: 'files',
    task: 'Read a text file in the terminal',
    summary:
      'cat prints the whole file. For a long one use less: arrows and Page Down to move, / to search, Q to leave.',
    command: 'cat FILE',
    variants: [
      {
        label: 'Scroll through a long file (Q to leave)',
        command: 'less FILE',
        keywords: ['less', 'pager', 'page through', 'how do i exit less', 'more', 'scroll up']
      },
      {
        label: 'Show the first 20 lines',
        command: 'head -n 20 FILE',
        keywords: ['head', 'top of file', 'beginning', 'preview a file', 'first rows']
      },
      {
        label: 'Show the last 20 lines',
        command: 'tail -n 20 FILE',
        keywords: ['tail', 'end of file', 'bottom', 'end of a log', 'final rows']
      }
    ],
    placeholders: { FILE: 'The file to read' },
    keywords: [
      'cat',
      'type',
      'read file',
      'show file',
      'print file',
      'view contents',
      'get-content',
      'less',
      'more',
      'head',
      'tail',
      'how do i exit less'
    ]
  },
  {
    id: 'sh-follow-log',
    shell: 'bash',
    category: 'files',
    task: 'Watch a log file as it grows',
    summary:
      'Prints the end of the file and then every new line as it is written. Ctrl+C stops watching.',
    command: 'tail -f FILE',
    placeholders: { FILE: 'The log file' },
    keywords: [
      'tail -f',
      'follow log',
      'watch file',
      'live log',
      'get-content -wait',
      'stream log',
      'monitor file',
      'log output'
    ]
  },
  {
    id: 'sh-make-file',
    shell: 'bash',
    category: 'files',
    task: 'Make a new empty file',
    summary:
      'Creates the file if it is missing. If it already exists its contents are left alone and only its date changes.',
    command: 'touch NAME',
    variants: [
      {
        label: 'Create a file with one line of text',
        command: 'echo "TEXT" > NAME',
        keywords: ['echo to a file', 'write text', 'overwrite', 'redirect', 'make with content']
      }
    ],
    placeholders: {
      NAME: 'The file to create, for example notes.txt',
      TEXT: 'The first line of the file'
    },
    danger:
      'The echo variant replaces everything in the file if it already exists. touch never does.',
    keywords: [
      'touch',
      'create file',
      'new file',
      'make file',
      'empty file',
      'new-item',
      'type nul',
      'blank file',
      'create .env'
    ]
  },
  {
    id: 'sh-edit-file',
    shell: 'bash',
    category: 'files',
    task: 'Edit a file in the terminal',
    summary:
      'nano is the friendly editor: type as normal, Ctrl+O then Enter saves, Ctrl+X leaves. The shortcuts are listed along the bottom, where ^ means Ctrl.',
    command: 'nano FILE',
    variants: [
      {
        label: 'Edit the file in VS Code',
        command: 'code FILE',
        keywords: ['vscode', 'gui editor', 'visual studio', 'open in an editor', 'code file']
      },
      {
        label: 'Quit vim without saving (Esc first)',
        command: ':q!',
        keywords: ['q!', 'exit vim', 'stuck in vim', 'vi', 'discard changes', 'get out of vim']
      }
    ],
    placeholders: { FILE: 'The file to edit (it is created if missing)' },
    keywords: [
      'nano',
      'edit file',
      'text editor',
      'notepad',
      'vim',
      'vi',
      'how do i exit vim',
      'quit vim',
      'save in nano',
      'open file to edit'
    ]
  },
  {
    id: 'sh-copy',
    shell: 'bash',
    category: 'files',
    task: 'Copy a file or a folder',
    summary:
      'Copies a file to a new name or into a folder. A folder needs -r ("and everything inside").',
    command: 'cp SOURCE DESTINATION',
    variants: [
      {
        label: 'Copy a whole folder',
        command: 'cp -r SOURCE DESTINATION',
        keywords: ['cp -r', 'recursive', 'copy a directory', 'xcopy', 'omitting directory']
      },
      {
        label: 'Ask before replacing anything',
        command: 'cp -i SOURCE DESTINATION',
        keywords: ['cp -i', 'prompt', 'confirm overwrite', 'interactive', 'do not clobber']
      }
    ],
    placeholders: {
      SOURCE: 'What to copy',
      DESTINATION: 'A folder to copy it into, or the new name'
    },
    keywords: [
      'cp',
      'copy',
      'copy file',
      'copy folder',
      'cp -r',
      'robocopy',
      'xcopy',
      'copy-item',
      'duplicate',
      'back up a file',
      'omitting directory'
    ],
    danger:
      'A file of the same name at the destination is replaced without asking, unless you add -i.'
  },
  {
    id: 'sh-move-rename',
    shell: 'bash',
    category: 'files',
    task: 'Move or rename a file or folder',
    summary:
      'Linux has one command for both: moving a file to a new name in the same folder IS renaming it.',
    command: 'mv SOURCE DESTINATION',
    variants: [
      {
        label: 'Ask before replacing an existing file',
        command: 'mv -i SOURCE DESTINATION',
        keywords: ['mv -i', 'prompt', 'confirm overwrite', 'interactive', 'safe move']
      }
    ],
    placeholders: {
      SOURCE: 'The file or folder as it is now',
      DESTINATION: 'The new name, or the folder to move it into'
    },
    keywords: [
      'mv',
      'move',
      'rename',
      'ren',
      'move-item',
      'rename-item',
      'change file name',
      'put file in folder',
      'relocate'
    ],
    danger:
      'A file of the same name at the destination is replaced without asking, unless you add -i.'
  },
  {
    id: 'sh-delete-file',
    shell: 'bash',
    category: 'files',
    task: 'Delete a file',
    summary:
      'Removes the file. A wildcard removes every match, so run ls with the same pattern first to see what it will take.',
    command: 'rm FILE',
    variants: [
      {
        label: 'Ask before deleting each file',
        command: 'rm -i FILE',
        keywords: ['rm -i', 'prompt', 'confirm', 'interactive', 'are you sure']
      },
      {
        label: 'Delete every file of one type here',
        command: 'rm *.EXT',
        keywords: ['rm *.log', 'wildcard', 'by extension', 'glob', 'clear out logs']
      }
    ],
    placeholders: { FILE: 'The file to delete', EXT: 'The file extension, for example log' },
    keywords: [
      'rm',
      'del',
      'erase',
      'delete file',
      'remove file',
      'remove-item',
      'get rid of file',
      'unlink'
    ],
    danger: 'There is no Recycle Bin in Linux. A deleted file is gone for good.'
  },
  {
    id: 'sh-find-file-by-name',
    shell: 'bash',
    category: 'files',
    task: 'Find a file by name in subfolders',
    summary:
      'Searches every subfolder. -iname ignores upper and lower case and the stars mean "anything", so part of the name is enough. Keep the quotes, or the shell expands the stars itself.',
    command: 'find . -iname "*NAME*"',
    variants: [
      {
        label: 'Find files only, hiding permission errors',
        command: 'find . -type f -iname "*NAME*" 2>/dev/null',
        keywords: ['2>/dev/null', 'hide errors', 'type f', 'quiet search', 'denied noise']
      },
      {
        label: 'Find files changed in the last day',
        command: 'find . -type f -mtime -1',
        keywords: ['mtime', 'modified today', 'recent files', 'last 24 hours', 'since yesterday']
      },
      {
        label: 'Find files changed in the last hour',
        command: 'find . -type f -mmin -60',
        keywords: ['mmin', 'just modified', 'recent edits', 'last 60 minutes', 'what i just saved']
      }
    ],
    placeholders: { NAME: 'Any part of the file name' },
    keywords: [
      'find',
      'find file',
      'search for file',
      'locate',
      'where is my file',
      'dir /s',
      'get-childitem -recurse',
      'recently changed files',
      'modified today',
      'wildcard'
    ]
  },
  {
    id: 'sh-biggest-files',
    shell: 'bash',
    category: 'files',
    task: 'Find the biggest files and folders here',
    summary:
      'Measures everything under this folder, sorts by size with the biggest on top, and keeps the first twenty.',
    command: 'du -ah . | sort -rh | head -n 20',
    keywords: [
      'big files',
      'biggest files',
      'largest files',
      'what is using disk space',
      'disk full',
      'no space left on device',
      'find large files',
      'clean up space',
      'du sort'
    ]
  },
  {
    id: 'sh-make-executable',
    shell: 'bash',
    category: 'files',
    task: 'Run a script that says "Permission denied"',
    summary:
      'A script has to be marked executable before ./ will run it. Do that once, then start it with ./ in front: Linux does not look in the current folder for commands by itself.',
    command: 'chmod +x SCRIPT',
    variants: [
      {
        label: 'Run the script with ./ in front',
        command: './SCRIPT',
        keywords: ['./script.sh', 'dot slash', 'execute', 'command not found', 'not on path']
      },
      {
        label: 'Run a script without marking it executable',
        command: 'bash SCRIPT',
        keywords: ['bash script.sh', 'sh script.sh', 'no chmod', 'one off', 'interpreter']
      }
    ],
    placeholders: { SCRIPT: 'The script file, for example build.sh' },
    keywords: [
      'chmod',
      'chmod +x',
      'permission denied',
      'make executable',
      'run script',
      'run sh file',
      'dot slash',
      './',
      'command not found script',
      'execute bit'
    ]
  },
  {
    id: 'sh-permissions',
    shell: 'bash',
    category: 'files',
    task: "See or fix a file's permissions",
    summary:
      'The first column of ls -l reads r (read), w (write), x (run) three times over: for the owner, the group, and everyone else. The name after it is the owner.',
    command: 'ls -l FILE',
    variants: [
      {
        label: 'Give yourself read and write on a folder',
        command: 'chmod -R u+rwX FOLDER',
        keywords: ['chmod -r', 'u+rwx', 'recursive chmod', 'eacces', 'access denied']
      },
      {
        label: 'Take back a folder owned by root',
        command: 'sudo chown -R $USER FOLDER',
        keywords: ['chown', 'ownership', '$user', 'sudo left it root', 'cannot write']
      },
      {
        label: 'Fix SSH key permissions',
        command: 'chmod 600 ~/.ssh/id_ed25519',
        keywords: ['600', 'unprotected private key file', 'id_ed25519', 'bad mode', 'key refused']
      }
    ],
    placeholders: { FILE: 'The file to look at', FOLDER: 'The folder to fix' },
    keywords: [
      'permissions',
      'chmod',
      'chown',
      'permission denied',
      'access denied',
      'read only',
      'owned by root',
      'eacces',
      'icacls',
      'unprotected private key file',
      'file mode'
    ]
  },
  {
    id: 'sh-line-endings',
    shell: 'bash',
    category: 'files',
    task: 'Fix a script broken by Windows line endings',
    summary:
      'A file saved on Windows ends its lines with CR LF, and bash reads the CR as part of the command: "bad interpreter: /bin/bash^M" or "$\'\\r\': command not found". This strips the CR from every line.',
    command: "sed -i 's/\\r$//' FILE",
    variants: [
      {
        label: 'Check a file for CRLF line endings',
        command: 'file FILE',
        keywords: ['file command', 'dos text', 'crlf terminators', 'detect line endings', 'is it dos']
      },
      {
        label: 'Convert with dos2unix (apt install dos2unix)',
        command: 'dos2unix FILE',
        keywords: ['dos2unix', 'crlf to lf', 'unix2dos', 'not installed', 'strip carriage returns']
      },
      {
        label: 'Stop git converting line endings on checkout',
        command: 'git config --global core.autocrlf input',
        keywords: ['core.autocrlf', 'git config', 'input', 'clone', 'every file changed']
      }
    ],
    placeholders: { FILE: 'The script or text file to fix' },
    keywords: [
      'line endings',
      'crlf',
      'lf',
      'carriage return',
      'bad interpreter',
      '^m',
      '\\r command not found',
      'dos2unix',
      'windows line endings',
      'script wont run in wsl',
      'autocrlf'
    ],
    danger:
      'The file is rewritten in place. Harmless for text and scripts, but never run it on a binary file.'
  },
  {
    id: 'sh-tar-extract',
    shell: 'bash',
    category: 'files',
    task: 'Unpack a .tar.gz archive',
    summary:
      'x extracts, f names the file, and modern tar works out the compression (.gz, .bz2, .xz) by itself.',
    command: 'tar -xf FILE',
    variants: [
      {
        label: 'Unpack into another folder',
        command: 'tar -xf FILE -C FOLDER',
        keywords: ['tar -c', 'destination', 'extract to', 'target directory', 'somewhere else']
      },
      {
        label: 'List what is inside the archive',
        command: 'tar -tf FILE',
        keywords: ['tar -tf', 'contents', 'peek', 'without extracting', 'preview']
      }
    ],
    placeholders: { FILE: 'The archive, for example site.tar.gz', FOLDER: 'Where to unpack it' },
    keywords: [
      'tar',
      'untar',
      'extract',
      'unpack',
      'tar.gz',
      'tgz',
      'tarball',
      'expand-archive',
      'decompress',
      'gunzip'
    ],
    danger: 'Files already there with the same names are replaced without asking.'
  },
  {
    id: 'sh-tar-create',
    shell: 'bash',
    category: 'files',
    task: 'Pack a folder into a .tar.gz archive',
    summary:
      'c creates, z compresses with gzip, f names the archive. The folder itself is left as it is.',
    command: 'tar -czf NAME.tar.gz FOLDER',
    placeholders: { NAME: 'What to call the archive', FOLDER: 'The folder to pack' },
    keywords: [
      'tar',
      'compress',
      'archive folder',
      'make tarball',
      'tar.gz',
      'compress-archive',
      'back up folder',
      'gzip'
    ],
    danger: 'An archive of the same name is replaced without asking.'
  },
  {
    id: 'sh-unzip',
    shell: 'bash',
    category: 'files',
    task: 'Unzip a .zip file',
    summary:
      'Unpacks into the folder named after -d, making it if needed. A stock Ubuntu has neither unzip nor zip: sudo apt install unzip zip.',
    command: 'unzip FILE -d FOLDER',
    variants: [
      {
        label: 'List what is inside the zip',
        command: 'unzip -l FILE',
        keywords: ['unzip -l', 'contents', 'peek', 'without extracting', 'preview']
      },
      {
        label: 'Make a zip from a folder',
        command: 'zip -r NAME.zip FOLDER',
        keywords: ['zip -r', 'compress a folder', 'create an archive', 'compress-archive', 'package']
      }
    ],
    placeholders: {
      FILE: 'The zip file, for example site.zip',
      FOLDER: 'Where to unpack, or what to pack',
      NAME: 'What to call the new zip'
    },
    keywords: [
      'unzip',
      'zip',
      'extract zip',
      'expand-archive',
      'compress-archive',
      'unzip command not found',
      'open zip',
      'decompress'
    ]
  },

  // ------------------------------------------------------------------- text
  {
    id: 'sh-grep-recursive',
    shell: 'bash',
    category: 'text',
    task: 'Find text inside files',
    summary:
      '-r searches this folder and every subfolder, -n shows the line number and -i ignores upper and lower case.',
    command: 'grep -rni "TEXT"',
    variants: [
      {
        label: 'List only the file names that match',
        command: 'grep -rli "TEXT"',
        keywords: ['grep -l', 'which files', 'no lines', 'just the paths', 'files containing']
      },
      {
        label: 'Search one type of file only',
        command: 'grep -rni --include="*.EXT" "TEXT"',
        keywords: ['--include', 'by extension', 'only ts files', 'glob filter', 'limit the search']
      },
      {
        label: 'Search but skip node_modules',
        command: 'grep -rni --exclude-dir=node_modules "TEXT"',
        keywords: ['--exclude-dir', 'ignore a folder', 'vendor', '.git', 'too many results']
      }
    ],
    placeholders: { TEXT: 'The words to look for', EXT: 'The file extension, for example ts' },
    keywords: [
      'grep',
      'grep -r',
      'findstr',
      'select-string',
      'search in files',
      'find text',
      'find string',
      'search code',
      'which file contains',
      'find word'
    ]
  },
  {
    id: 'sh-filter-output',
    shell: 'bash',
    category: 'text',
    task: 'Keep only the output lines that match',
    summary: 'Pipes a command into grep so a long list is cut down to the lines you care about.',
    command: 'ls -la | grep -i "TEXT"',
    variants: [
      {
        label: 'Hide the output lines that match',
        command: 'ls -la | grep -iv "TEXT"',
        keywords: ['grep -v', 'invert', 'exclude', 'filter out', 'everything except']
      }
    ],
    placeholders: { TEXT: 'The word to keep' },
    keywords: [
      'filter output',
      'grep',
      'pipe to grep',
      'search output',
      'where-object',
      'findstr',
      'only matching lines',
      'too much output'
    ]
  },
  {
    id: 'sh-count-lines',
    shell: 'bash',
    category: 'text',
    task: 'Count lines, words or files',
    summary:
      'wc -l counts lines, and anything can be piped into it: count the files here, the matches of a grep, and so on.',
    command: 'wc -l FILE',
    variants: [
      {
        label: 'Count the files in this folder',
        command: 'ls | wc -l',
        keywords: ['ls | wc -l', 'how many files', 'file count', 'number of items', 'tally']
      },
      {
        label: 'Count words instead of lines',
        command: 'wc -w FILE',
        keywords: ['wc -w', 'word count', 'how many words', 'wc', 'text stats']
      }
    ],
    placeholders: { FILE: 'The file to count' },
    keywords: [
      'wc',
      'count lines',
      'line count',
      'how many lines',
      'how many files',
      'word count',
      'measure-object',
      'find /c'
    ]
  },
  {
    id: 'sh-compare-files',
    shell: 'bash',
    category: 'text',
    task: 'Compare two files',
    summary:
      'Lines starting - are only in the first file, lines starting + only in the second. No output at all means they are the same.',
    command: 'diff -u FILE1 FILE2',
    variants: [
      {
        label: 'Compare two whole folders',
        command: 'diff -rq FOLDER1 FOLDER2',
        keywords: ['diff -rq', 'recursive diff', 'which files differ', 'directories', 'side by side']
      }
    ],
    placeholders: {
      FILE1: 'The first file',
      FILE2: 'The second file',
      FOLDER1: 'The first folder',
      FOLDER2: 'The second folder'
    },
    keywords: [
      'diff',
      'compare files',
      'fc',
      'compare-object',
      'what changed',
      'are these the same',
      'differences',
      'compare folders'
    ]
  },
  {
    id: 'sh-replace-text',
    shell: 'bash',
    category: 'text',
    task: 'Replace a word throughout a file',
    summary:
      's/OLD/NEW/g swaps every OLD for NEW, and -i writes the result back into the file. Run the preview variant first: it only prints.',
    command: "sed -i 's/OLD/NEW/g' FILE",
    variants: [
      {
        label: 'Preview the replacement without changing the file',
        command: "sed 's/OLD/NEW/g' FILE",
        keywords: ['sed without -i', 'dry run', 'print only', 'safe check', 'try it first']
      }
    ],
    placeholders: {
      OLD: 'The text to find (a / or a . in it needs a backslash in front)',
      NEW: 'What to put in its place',
      FILE: 'The file to change'
    },
    keywords: [
      'sed',
      'replace text',
      'find and replace',
      'search and replace',
      'substitute',
      'rename variable',
      '-replace',
      'change word in file'
    ],
    danger:
      'The file is changed in place with no backup. Use sed -i.bak to keep the original as FILE.bak.'
  },
  {
    id: 'sh-sort-unique',
    shell: 'bash',
    category: 'text',
    task: 'Sort lines and count the duplicates',
    summary:
      'Sorts the lines, folds identical ones together with a count, and puts the most frequent on top.',
    command: 'sort FILE | uniq -c | sort -nr',
    variants: [
      {
        label: 'Sort and remove duplicate lines',
        command: 'sort -u FILE',
        keywords: ['sort -u', 'dedupe', 'unique', 'distinct', 'no repeats']
      }
    ],
    placeholders: { FILE: 'The file to sort' },
    keywords: [
      'sort',
      'uniq',
      'unique lines',
      'remove duplicates',
      'count occurrences',
      'most common',
      'sort-object',
      'group-object',
      'dedupe'
    ]
  },
  {
    id: 'sh-pick-column',
    shell: 'bash',
    category: 'text',
    task: 'Pick out one column of output',
    summary:
      'awk splits each line on spaces and $2 is the second piece. For comma-separated text use cut.',
    command: "ps aux | awk '{print $2}'",
    variants: [
      {
        label: 'Pick a column from comma-separated text',
        command: "cut -d',' -f2 FILE",
        keywords: ['cut -d', 'csv field', 'delimiter', 'second field', 'split on a comma']
      }
    ],
    placeholders: { FILE: 'A CSV file' },
    keywords: [
      'awk',
      'cut',
      'column',
      'field',
      'extract column',
      'select-object',
      'csv column',
      'split line',
      'second word'
    ]
  },

  // -------------------------------------------------------------- processes
  {
    id: 'sh-list-processes',
    shell: 'bash',
    category: 'processes',
    task: 'See what is running',
    summary:
      'Lists every process with its owner, its PID (the number you need to stop it), and its CPU and memory share.',
    command: 'ps aux',
    variants: [
      {
        label: 'Find a running program by name',
        command: 'ps aux | grep -i NAME',
        keywords: ['is node running', 'search processes', 'filter the list', 'which pid', 'ps grep']
      },
      {
        label: 'Show PIDs and command lines by name',
        command: 'pgrep -fl NAME',
        keywords: ['pgrep', 'pgrep -fl', 'process id', 'get the pid', 'what arguments']
      },
      {
        label: 'Live view of the busiest processes',
        command: 'top',
        keywords: ['top', 'htop', 'task manager', 'cpu usage', 'q to quit', 'monitor']
      }
    ],
    placeholders: { NAME: 'The program, for example node' },
    keywords: [
      'ps',
      'ps aux',
      'tasklist',
      'get-process',
      'running programs',
      'processes',
      'task manager',
      'top',
      'htop',
      'is node running',
      'pid',
      'cpu usage'
    ]
  },
  {
    id: 'sh-kill-process',
    shell: 'bash',
    category: 'processes',
    task: 'Stop a program by its PID or name',
    summary:
      'Plain kill asks the program to stop and lets it tidy up. Reach for -9 only when it ignores that: -9 cannot be refused and nothing gets saved.',
    command: 'kill PID',
    variants: [
      {
        label: 'Force-stop a program that ignores kill',
        command: 'kill -9 PID',
        keywords: ['kill -9', 'sigkill', 'wont die', 'unresponsive', 'hard kill', 'force quit']
      },
      {
        label: 'Stop every process with one name',
        command: 'pkill NAME',
        keywords: ['pkill', 'killall', 'kill by name', 'kill node', 'end all instances']
      },
      {
        label: 'Stop processes by their command line',
        command: 'pkill -f TEXT',
        keywords: ['pkill -f', 'full command line', 'kill vite', 'match arguments', 'pattern']
      }
    ],
    placeholders: {
      PID: 'The number from ps aux or lsof',
      NAME: 'The program name, for example node',
      TEXT: 'Part of the command line, for example vite'
    },
    keywords: [
      'kill',
      'kill -9',
      'pkill',
      'killall',
      'taskkill',
      'stop-process',
      'stop program',
      'end task',
      'frozen',
      'not responding',
      'kill node'
    ],
    danger:
      'The program stops without the chance to save, and pkill stops EVERY match, not just one.'
  },
  {
    id: 'sh-port-in-use',
    shell: 'bash',
    category: 'processes',
    task: 'Find what is using a port',
    summary:
      'Names the program and PID holding the port. No output means nothing of YOURS holds it: put sudo in front to see other users. The ss variant is always installed, should lsof be missing.',
    command: 'lsof -i :PORT',
    variants: [
      {
        label: 'List every listening port and program',
        command: 'ss -ltnp',
        keywords: ['ss -ltnp', 'netstat -ano', 'open ports', 'what is listening', 'sockets']
      },
      {
        label: 'Check one port with ss',
        command: 'ss -ltnp | grep :PORT',
        keywords: ['ss', 'port 3000', 'lsof missing', 'single port', 'who holds it']
      }
    ],
    placeholders: { PORT: 'The port number, for example 3000' },
    keywords: [
      'port in use',
      'port already in use',
      'eaddrinuse',
      'address already in use',
      'what is using port 3000',
      'lsof',
      'lsof -i',
      'netstat',
      'ss',
      'listening ports',
      'free a port'
    ]
  },
  {
    id: 'sh-cancel-command',
    shell: 'bash',
    category: 'processes',
    task: 'Stop a command that is still running',
    summary:
      'Not a command but a key: hold Ctrl and press C. If the program ignores it, Ctrl+Z parks it and hands the prompt back, and the variant then finishes it off.',
    command: 'Ctrl+C',
    variants: [
      {
        label: 'Force-stop a job parked with Ctrl+Z',
        command: 'kill -9 %1',
        keywords: ['kill -9 %1', 'job number', 'stopped jobs', 'suspended', 'background job']
      }
    ],
    keywords: [
      'cancel',
      'stop',
      'abort',
      'interrupt',
      'ctrl c',
      'ctrl z',
      'how do i stop this',
      'stuck',
      'hung',
      'wont stop',
      'exit running command',
      'sigint'
    ],
    danger: 'The kill variant stops the parked program without the chance to save.'
  },
  {
    id: 'sh-background-jobs',
    shell: 'bash',
    category: 'processes',
    task: 'Run something in the background',
    summary:
      'An & at the end starts the command and hands the prompt straight back. jobs lists what this shell has running, and fg brings one to the front again.',
    command: 'npm run dev &',
    variants: [
      {
        label: 'List the background jobs of this shell',
        command: 'jobs',
        keywords: ['job numbers', 'what is running here', 'stopped', 'suspended', 'my tasks']
      },
      {
        label: 'Bring a background job to the front',
        command: 'fg',
        keywords: ['fg', 'foreground', 'resume', 'unpause', 'come back to it']
      },
      {
        label: 'Resume a parked job in the background',
        command: 'bg',
        keywords: ['bg', 'ctrl z', 'continue', 'suspended', 'carry on']
      },
      {
        label: 'Keep a command running after the terminal closes',
        command: 'nohup npm run dev > output.log 2>&1 &',
        keywords: ['nohup', 'disown', 'survive logout', 'detach', 'ssh disconnect', 'long job']
      }
    ],
    danger: 'The nohup variant replaces output.log if a file of that name is already there.',
    keywords: [
      'background',
      'ampersand',
      'jobs',
      'fg',
      'bg',
      'nohup',
      'start-job',
      'start /b',
      'keep running after close',
      'ctrl z',
      'suspended',
      'there are stopped jobs'
    ]
  },

  // ---------------------------------------------------------------- network
  {
    id: 'sh-ip-address',
    shell: 'bash',
    category: 'network',
    task: 'Find my IP address',
    summary:
      "Prints this machine's addresses. Under WSL that is the Linux side's own address, which is usually not the one Windows has on your network.",
    command: 'hostname -I',
    variants: [
      {
        label: 'Show every network interface',
        command: 'ip addr',
        keywords: ['ip addr', 'ifconfig', 'eth0', 'adapters', 'mac address', 'subnet']
      },
      {
        label: 'Show the public IP the internet sees',
        command: 'curl -s https://ifconfig.me',
        keywords: ['public ip', 'external ip', 'ifconfig.me', 'whats my ip', 'wan']
      }
    ],
    keywords: [
      'ip address',
      'my ip',
      'local ip',
      'ipconfig',
      'ifconfig',
      'ip addr',
      'public ip',
      'wsl ip',
      'network interface'
    ]
  },
  {
    id: 'sh-ping',
    shell: 'bash',
    category: 'network',
    task: 'Check whether a site can be reached',
    summary:
      'Linux ping goes on for ever unless told otherwise, so -c 4 sends four packets and stops.',
    command: 'ping -c 4 HOST',
    variants: [
      {
        label: 'Look up a name in DNS',
        command: 'getent hosts HOST',
        keywords: ['getent hosts', 'nslookup', 'dig', 'resolve', 'etc hosts', 'dns']
      }
    ],
    placeholders: { HOST: 'A name or an address, for example github.com' },
    keywords: [
      'ping',
      'is the internet working',
      'connection test',
      'test-connection',
      'dns lookup',
      'nslookup',
      'site down',
      'no internet',
      'ping never stops',
      'temporary failure in name resolution'
    ]
  },
  {
    id: 'sh-download',
    shell: 'bash',
    category: 'network',
    task: 'Download a file from a URL',
    summary:
      '-L follows redirects (most download links have one) and -O keeps the name the file has on the server.',
    command: 'curl -L -O URL',
    variants: [
      {
        label: 'Download and choose the saved name',
        command: 'curl -L -o FILE URL',
        keywords: ['curl -o', 'save as', 'output file', 'rename a download', 'my own name']
      },
      {
        label: 'Download with wget instead',
        command: 'wget URL',
        keywords: ['wget', 'wget url', 'fetch', 'no curl', 'another downloader']
      }
    ],
    placeholders: {
      URL: 'The full address, starting https://',
      FILE: 'What to call the saved file'
    },
    danger:
      'curl replaces a file of the same name without a question. wget keeps it and saves beside it.',
    keywords: [
      'curl',
      'wget',
      'download',
      'fetch url',
      'invoke-webrequest',
      'save file from internet',
      'get file',
      'download script'
    ]
  },
  {
    id: 'sh-check-server',
    shell: 'bash',
    category: 'network',
    task: 'Check a website or local server answers',
    summary:
      '-I asks for the headers only, so the first line is the verdict: 200 is fine, 404 not found, "Connection refused" means nothing is listening there.',
    command: 'curl -I http://localhost:PORT',
    variants: [
      {
        label: 'Show the reply body and headers',
        command: 'curl -i URL',
        keywords: ['curl -i', 'response body', 'see the json', 'full response', 'http headers']
      },
      {
        label: 'Send a JSON POST to an API',
        command: 'curl -X POST -H "Content-Type: application/json" -d \'{"KEY":"VALUE"}\' URL',
        keywords: ['curl -x post', 'post json', 'api request', 'content-type', 'test an endpoint']
      }
    ],
    placeholders: {
      PORT: 'The port your server runs on, for example 3000',
      URL: 'The full address',
      KEY: 'A field name in the JSON',
      VALUE: 'Its value'
    },
    keywords: [
      'curl',
      'http request',
      'test api',
      'is my server running',
      'localhost',
      'connection refused',
      'status code',
      'post json',
      'invoke-restmethod',
      'headers'
    ]
  },
  {
    id: 'sh-ssh',
    shell: 'bash',
    category: 'network',
    task: 'Log in to another machine over SSH',
    summary: 'Opens a shell on the remote machine. exit, or Ctrl+D, brings you back.',
    command: 'ssh USER@HOST',
    variants: [
      {
        label: 'Connect on a port other than 22',
        command: 'ssh -p PORT USER@HOST',
        keywords: ['ssh -p', 'custom port', 'port 2222', 'non standard', 'connection refused']
      },
      {
        label: 'Connect with a particular key file',
        command: 'ssh -i ~/.ssh/KEYFILE USER@HOST',
        keywords: ['ssh -i', 'identity file', 'private key', 'pem', 'id_ed25519']
      },
      {
        label: 'Copy a file to the other machine',
        command: 'scp FILE USER@HOST:FOLDER',
        keywords: ['scp', 'upload', 'transfer', 'remote copy', 'sftp', 'send to server']
      }
    ],
    placeholders: {
      USER: 'Your user name on the other machine',
      HOST: 'Its name or address',
      PORT: 'The SSH port',
      KEYFILE: 'The private key, for example id_ed25519',
      FILE: 'The local file to send',
      FOLDER: 'The folder on the other machine'
    },
    keywords: [
      'ssh',
      'remote login',
      'connect to server',
      'scp',
      'copy file to server',
      'remote shell',
      'vps',
      'putty',
      'permission denied publickey'
    ]
  },
  {
    id: 'sh-ssh-keygen',
    shell: 'bash',
    category: 'network',
    task: 'Make an SSH key for GitHub',
    summary:
      'Creates a key pair in ~/.ssh and asks where to save it and for an optional passphrase. The .pub half is the one you paste into GitHub; the other half never leaves this machine.',
    command: 'ssh-keygen -t ed25519 -C "EMAIL"',
    variants: [
      {
        label: 'Print the public key, ready to paste',
        command: 'cat ~/.ssh/id_ed25519.pub',
        keywords: ['id_ed25519.pub', 'copy the key', 'pub file', 'add to github', 'deploy key']
      },
      {
        label: 'Test the connection to GitHub',
        command: 'ssh -T git@github.com',
        keywords: ['ssh -t', 'git@github.com', 'permission denied publickey', 'does my key work']
      }
    ],
    placeholders: { EMAIL: 'A label for the key, usually your email address' },
    keywords: [
      'ssh key',
      'ssh-keygen',
      'github ssh',
      'public key',
      'id_ed25519',
      'id_rsa',
      'permission denied publickey',
      'deploy key',
      'generate key'
    ],
    danger:
      'If it asks "Overwrite (y/n)?", a key is already there: answering y destroys it, and everything that trusted it stops working.'
  },

  // ----------------------------------------------------------------- system
  {
    id: 'sh-disk-free',
    shell: 'bash',
    category: 'system',
    task: 'See how much disk space is free',
    summary: 'One line per disk with its size, what is used and what is left, in K, M and G.',
    command: 'df -h',
    variants: [
      {
        label: 'Free space on the Windows C: drive',
        command: 'df -h /mnt/c',
        keywords: ['df /mnt/c', 'host disk', 'wsl', 'c drive', 'how full is windows']
      }
    ],
    keywords: [
      'df',
      'df -h',
      'disk space',
      'free space',
      'drive full',
      'no space left on device',
      'get-psdrive',
      'storage',
      'how much space left'
    ]
  },
  {
    id: 'sh-memory',
    shell: 'bash',
    category: 'system',
    task: 'See how much memory is in use',
    summary:
      'The "available" column is the one that matters: it is what programs can still have. Under WSL the total is what Windows lends to Linux, not the whole machine.',
    command: 'free -h',
    keywords: [
      'free',
      'memory',
      'ram',
      'how much ram',
      'out of memory',
      'swap',
      'memory usage',
      'killed',
      'wsl memory'
    ]
  },
  {
    id: 'sh-env-show',
    shell: 'bash',
    category: 'system',
    task: 'See my environment variables',
    summary:
      'printenv lists them all. A single value is read with a dollar sign in front of its name.',
    command: 'printenv',
    variants: [
      {
        label: 'Show the value of one variable',
        command: 'echo "$NAME"',
        keywords: ['echo $name', 'read a variable', 'print the value', 'is it set', 'dollar sign']
      },
      {
        label: 'Search the environment variables for a word',
        command: 'printenv | grep -i TEXT',
        keywords: ['printenv grep', 'find a variable', 'is my api key set', 'which variables']
      }
    ],
    placeholders: {
      NAME: 'The variable, for example HOME',
      TEXT: 'Part of the name, for example api'
    },
    keywords: [
      'environment variables',
      'env',
      'printenv',
      'set',
      'show variable',
      '$env:',
      'is my api key set',
      'echo variable',
      'dollar sign'
    ]
  },
  {
    id: 'sh-env-set',
    shell: 'bash',
    category: 'system',
    task: 'Set an environment variable',
    summary:
      'export sets it for this shell and everything started from it, until the window closes. No spaces around the =. The ~/.bashrc variant makes it permanent for new shells.',
    command: 'export NAME="VALUE"',
    variants: [
      {
        label: 'Set a variable for one command only',
        command: 'NAME="VALUE" COMMAND',
        keywords: ['inline', 'prefix assignment', 'temporary', 'one off', 'just this run']
      },
      {
        label: 'Set a variable permanently in ~/.bashrc',
        command: 'echo \'export NAME="VALUE"\' >> ~/.bashrc',
        keywords: ['bashrc', 'permanent', 'every new shell', 'survives a restart', 'startup file']
      },
    ],
    placeholders: {
      NAME: 'The variable name, for example ANTHROPIC_API_KEY',
      VALUE: 'Its value',
      COMMAND: 'The command that should see it'
    },
    keywords: [
      'export',
      'set environment variable',
      'env var',
      'api key',
      'setx',
      '$env:',
      'permanent variable',
      'bashrc',
      'variable gone after restart',
      'command not found after ='
    ]
  },
  {
    id: 'sh-add-to-path',
    shell: 'bash',
    category: 'system',
    task: 'Add a folder to PATH',
    summary:
      'PATH is the list of folders searched for commands, separated by colons. This adds one for the current shell; the variant saves it for every new one. Keep the $PATH part, or every other command disappears.',
    command: 'export PATH="$PATH:FOLDER"',
    variants: [
      {
        label: 'Add to PATH permanently in ~/.bashrc',
        command: 'echo \'export PATH="$PATH:FOLDER"\' >> ~/.bashrc',
        keywords: ['bashrc', 'every new shell', 'survives a restart', 'startup file', 'for good']
      },
      {
        label: 'List the folders on PATH, one per line',
        command: "echo \"$PATH\" | tr ':' '\\n'",
        keywords: ['echo $path', 'tr', 'show the path', 'entries', 'is my folder there']
      }
    ],
    placeholders: { FOLDER: 'The folder that holds the program, for example $HOME/.local/bin' },
    keywords: [
      'path',
      'add to path',
      'export path',
      'command not found',
      'not on path',
      '.local/bin',
      'edit path',
      '$env:path',
      'bashrc',
      'installed but not found'
    ]
  },
  {
    id: 'sh-which',
    shell: 'bash',
    category: 'system',
    task: 'Find where a program is installed',
    summary:
      'Prints the full path of what would run if you typed that name. No output means it is not on PATH. type also knows about aliases and built-ins.',
    command: 'which NAME',
    variants: [
      {
        label: 'Check aliases and shell built-ins too',
        command: 'type NAME',
        keywords: ['type', 'builtin', 'command -v', 'what actually runs', 'shell function']
      },
      {
        label: 'List every match on PATH in order',
        command: 'which -a NAME',
        keywords: ['which -a', 'duplicates', 'shadowed', 'multiple versions', 'wrong version']
      }
    ],
    placeholders: { NAME: 'The command, for example node or python3' },
    keywords: [
      'which',
      'where',
      'type',
      'command -v',
      'get-command',
      'find program',
      'is it installed',
      'which version runs',
      'path of executable'
    ]
  },
  {
    id: 'sh-versions',
    shell: 'bash',
    category: 'system',
    task: 'See which Linux version this is',
    summary:
      'Names the distribution and its release, which is what an install guide means by "Ubuntu 24.04".',
    command: 'lsb_release -a',
    variants: [
      {
        label: 'Read the version from /etc/os-release',
        command: 'cat /etc/os-release',
        keywords: ['os-release', 'distro name', 'version id', 'any distribution', 'debian']
      },
      {
        label: 'Show the kernel, and whether this is WSL',
        command: 'uname -a',
        keywords: ['uname -a', 'kernel', 'am i in wsl', 'wsl2', 'microsoft', 'architecture']
      },
      {
        label: 'Show which user I am',
        command: 'whoami',
        keywords: ['whoami', 'current user', 'username', 'my account', 'am i root']
      }
    ],
    keywords: [
      'linux version',
      'ubuntu version',
      'lsb_release',
      'uname',
      'os-release',
      'which distro',
      'am i in wsl',
      'whoami',
      'ver',
      'systeminfo'
    ]
  },
  {
    id: 'sh-sudo',
    shell: 'bash',
    category: 'system',
    task: 'Run a command as administrator',
    summary:
      'sudo runs ONE command as root and asks for YOUR Linux password (the one chosen when Ubuntu was set up). Nothing appears while you type it; that is normal.',
    command: 'sudo COMMAND',
    variants: [
      {
        label: 'Run the previous command again, with sudo',
        command: 'sudo !!',
        keywords: [
          'sudo !!',
          'bang bang',
          'repeat as root',
          'rerun as root',
          'forgot sudo',
          'retry elevated',
          'try again'
        ]
      }
    ],
    placeholders: { COMMAND: 'The command that was refused' },
    keywords: [
      'sudo',
      'run as admin',
      'administrator',
      'root',
      'permission denied',
      'access denied',
      'are you root',
      'password not showing',
      'elevated',
      'runas'
    ],
    danger:
      'Root can change or delete anything in Linux. Only put sudo in front of a command you understand.'
  },

  // --------------------------------------------------------------- packages
  {
    id: 'sh-apt-install',
    shell: 'bash',
    category: 'packages',
    task: 'Install a program or package on Ubuntu',
    summary:
      'apt update refreshes the list of what exists (a fresh WSL has none, so installs fail without it), then install fetches the program.',
    command: 'sudo apt update && sudo apt install -y NAME',
    placeholders: { NAME: 'The package, for example unzip or build-essential' },
    keywords: [
      'apt',
      'apt install',
      'apt-get',
      'install program',
      'install a package',
      'winget',
      'command not found',
      'unable to locate package',
      'package manager',
      'choco'
    ]
  },
  {
    id: 'sh-apt-upgrade',
    shell: 'bash',
    category: 'packages',
    task: 'Update everything installed on Ubuntu',
    summary:
      'Refreshes the package list, then upgrades what has a newer version. It shows the list and asks before going ahead.',
    command: 'sudo apt update && sudo apt upgrade',
    variants: [
      {
        label: 'Remove packages nothing needs any more',
        command: 'sudo apt autoremove',
        keywords: ['apt autoremove', 'orphaned', 'clean up', 'free disk space', 'old dependencies']
      }
    ],
    keywords: [
      'apt upgrade',
      'apt update',
      'update ubuntu',
      'update packages',
      'security updates',
      'winget upgrade',
      'upgrade all',
      'system update'
    ]
  },
  {
    id: 'sh-apt-search',
    shell: 'bash',
    category: 'packages',
    task: 'Find a package, or check what is installed',
    summary: 'Searches the names and descriptions of everything apt can install.',
    command: 'apt search TEXT',
    variants: [
      {
        label: 'Check whether a package is installed',
        command: 'apt list --installed 2>/dev/null | grep -i NAME',
        keywords: ['apt list', 'dpkg -l', 'which version', 'is it there', 'already have it']
      },
      {
        label: 'Read what a package is before installing',
        command: 'apt show NAME',
        keywords: ['apt show', 'description', 'dependencies', 'what does it do', 'package details']
      }
    ],
    placeholders: { TEXT: 'A word describing what you need', NAME: 'The package name' },
    keywords: [
      'apt search',
      'find package',
      'is it installed',
      'apt list',
      'apt show',
      'dpkg -l',
      'winget search',
      'package name',
      'installed version'
    ]
  },
  {
    id: 'sh-apt-remove',
    shell: 'bash',
    category: 'packages',
    task: 'Uninstall a program on Ubuntu',
    summary:
      'Removes the program and keeps its settings, in case it comes back. It lists what will go and asks first.',
    command: 'sudo apt remove NAME',
    variants: [
      {
        label: 'Remove a package and its settings',
        command: 'sudo apt purge NAME',
        keywords: ['apt purge', 'purge', 'delete the config', 'completely', 'system-wide settings']
      }
    ],
    placeholders: { NAME: 'The package to remove' },
    keywords: [
      'apt remove',
      'uninstall',
      'remove package',
      'apt purge',
      'winget uninstall',
      'delete program',
      'get rid of package'
    ],
    danger:
      'The program is uninstalled, and purge deletes its configuration as well. Read the list apt shows before answering y.'
  },

  // Python, Ubuntu's way (2026-10-13): python3, never python, which a stock
  // Ubuntu does not have, and a venv first, since Ubuntu's own Python refuses
  // pip installs outside one (PEP 668, "externally-managed-environment"). The
  // Windows spellings are the 'windows' entries in packages.ts.
  {
    id: 'sh-python-venv',
    shell: 'bash',
    category: 'packages',
    task: 'Create and activate a Python virtual environment',
    summary:
      "A venv is a private set of Python packages for ONE project, kept in a .venv folder. Ubuntu's own Python refuses pip installs outside one, so make one per project and activate it in every new tab: the prompt shows (.venv) while it is on.",
    command: 'python3 -m venv .venv',
    variants: [
      {
        label: 'Activate the venv',
        command: 'source .venv/bin/activate',
        keywords: ['source activate', 'bin/activate', 'turn the venv on', 'prompt shows .venv', 'every new tab']
      },
      {
        label: 'Install the Python venv module',
        command: 'sudo apt install -y python3-venv',
        keywords: ['ensurepip is not available', 'no module named venv', 'venv missing', 'python3-venv', 'apt']
      },
      {
        label: 'Deactivate the venv',
        command: 'deactivate',
        keywords: ['turn it off', 'leave the environment', 'exit venv', 'back to system python']
      }
    ],
    keywords: [
      'venv',
      'virtual environment',
      'virtualenv',
      'activate',
      'isolated python',
      'externally-managed-environment',
      'externally managed',
      'python environment',
      'pip installs in the wrong place',
      'no module named',
      'deactivate',
      'python3'
    ]
  },
  {
    id: 'sh-pip-install',
    shell: 'bash',
    category: 'packages',
    task: 'Install a Python package (pip)',
    summary:
      'Installs into the active virtual environment: activate one first, since Ubuntu refuses pip installs into its own Python. Spelling it python3 -m pip makes sure the pip belongs to the Python you are running.',
    command: 'python3 -m pip install PACKAGE',
    variants: [
      {
        label: 'Install an exact Python package version',
        command: 'python3 -m pip install PACKAGE==VERSION',
        keywords: ['pin a version', 'double equals', 'downgrade', 'older release', 'specific release']
      },
      {
        label: 'Upgrade a package you already have',
        command: 'python3 -m pip install --upgrade PACKAGE',
        keywords: ['pip -u', 'newer version', 'update a library', 'bring it up to date']
      },
      {
        label: 'Uninstall a Python package',
        command: 'python3 -m pip uninstall PACKAGE',
        keywords: ['remove a library', 'delete a package', 'pip remove', 'clean up the venv']
      }
    ],
    placeholders: { PACKAGE: 'The package name on pypi.org, such as requests', VERSION: 'A version number, such as 2.32.0' },
    keywords: [
      'pip install',
      'install a python library',
      'modulenotfounderror',
      'no module named',
      'importerror',
      'pip3',
      'pypi',
      'python package',
      'externally-managed-environment',
      'pip is not found'
    ]
  },
  {
    id: 'sh-pip-requirements',
    shell: 'bash',
    category: 'packages',
    task: 'Install everything in requirements.txt (pip)',
    summary:
      'Python projects list what they need in requirements.txt. After cloning one: create and activate a venv, then run this.',
    command: 'python3 -m pip install -r requirements.txt',
    keywords: [
      'requirements.txt',
      'install requirements',
      'install dependencies python',
      'after cloning',
      'set up a python project',
      'pip -r',
      'project dependencies',
      'pyproject'
    ]
  },
  {
    id: 'sh-pip-freeze',
    shell: 'bash',
    category: 'packages',
    task: 'Save the installed packages to requirements.txt',
    summary:
      'Writes every package in the active environment, with its exact version, to requirements.txt, so someone else (or you, later) can recreate it. Do it inside the venv.',
    command: 'python3 -m pip freeze > requirements.txt',
    variants: [
      {
        label: 'Print the package list without writing a file',
        command: 'python3 -m pip freeze',
        keywords: ['pip freeze', 'see the versions', 'without overwriting requirements.txt', 'check before saving']
      }
    ],
    keywords: [
      'freeze',
      'pip freeze',
      'export dependencies',
      'create requirements.txt',
      'lock versions',
      'save packages',
      'pin versions',
      'share my environment'
    ],
    danger: 'An existing requirements.txt in this folder is overwritten.'
  },
  {
    id: 'sh-pip-list',
    shell: 'bash',
    category: 'packages',
    task: 'See which Python packages are installed (pip)',
    summary: 'Lists the packages in the active environment with their versions.',
    command: 'python3 -m pip list',
    variants: [
      {
        label: 'List Python packages with an update available',
        command: 'python3 -m pip list --outdated',
        keywords: ['out of date', 'newer versions', 'check for upgrades', 'what needs updating']
      },
      {
        label: "Show a package's version, location and dependencies",
        command: 'python3 -m pip show PACKAGE',
        keywords: ['pip show', 'where is it installed', 'required-by', 'site-packages']
      },
      {
        label: 'Which Python and pip this shell uses',
        command: 'python3 -m pip --version',
        keywords: ['pip version', 'wrong interpreter', 'am i in the venv', 'python3 vs python']
      }
    ],
    placeholders: { PACKAGE: 'The package name' },
    keywords: [
      'pip list',
      'installed python packages',
      'which version of a library',
      'is it installed',
      'outdated python packages',
      'pip show',
      'which python',
      'python version'
    ]
  },
  // ------------------------------------------------------------------ shell
  {
    id: 'sh-clear',
    shell: 'bash',
    category: 'shell',
    task: 'Clear the screen',
    summary:
      'Wipes what is on screen; Ctrl+L does the same from the keyboard. Neither touches your files or history.',
    command: 'clear',
    variants: [
      {
        label: 'Reset a terminal printing garbage',
        command: 'reset',
        keywords: ['stty sane', 'garbled', 'weird characters', 'broken display', 'binary output']
      }
    ],
    keywords: [
      'clear',
      'cls',
      'clear screen',
      'clear-host',
      'ctrl l',
      'wipe terminal',
      'too much text',
      'garbled terminal',
      'reset'
    ]
  },
  {
    id: 'sh-history',
    shell: 'bash',
    category: 'shell',
    task: 'See or search the commands I have typed',
    summary:
      'Lists your commands, numbered, across sessions. Ctrl+R is the keyboard way: press it, type a few letters, and it finds the last command containing them (Ctrl+R again for older, Enter to run).',
    command: 'history',
    variants: [
      {
        label: 'Search the history for a word',
        command: 'history | grep -i TEXT',
        keywords: ['history grep', 'find an old command', 'what did i type', 'recall', 'that one']
      },
      {
        label: 'Show the last twenty commands',
        command: 'history | tail -n 20',
        keywords: ['recent commands', 'last 20', 'what i just ran', 'tail', 'lately']
      },
      {
        label: 'Run a numbered command again',
        command: '!N',
        keywords: ['!n', 'bang number', 'rerun', 'repeat', 'from the history list']
      }
    ],
    placeholders: {
      TEXT: 'Part of the command you are looking for',
      N: 'The number shown beside the command'
    },
    keywords: [
      'history',
      'previous commands',
      'what did i type',
      'ctrl r',
      'reverse search',
      'get-history',
      'doskey /history',
      'recall command',
      'up arrow',
      'bash_history'
    ]
  },
  {
    id: 'sh-help',
    shell: 'bash',
    category: 'shell',
    task: 'Get help on a command',
    summary:
      'man opens the full manual (arrows to scroll, / to search, Q to leave). --help is the short version, and works for nearly everything.',
    command: 'man COMMAND',
    variants: [
      {
        label: "Show a command's short help",
        command: 'tar --help',
        keywords: ['--help', '-h', 'usage', 'options', 'flags', 'quick reference']
      },
      {
        label: 'Help on a bash built-in (cd, export)',
        command: 'help COMMAND',
        keywords: ['builtin', 'no man page', 'shell builtin', 'alias', 'nothing appropriate']
      },
      {
        label: 'Find commands by what they do',
        command: 'man -k TEXT',
        keywords: ['man -k', 'apropos', 'search the manuals', 'which tool', 'i forget the name']
      }
    ],
    placeholders: {
      COMMAND: 'The command to read about, for example tar',
      TEXT: 'A word such as compress'
    },
    keywords: [
      'man',
      'help',
      '--help',
      'manual',
      'get-help',
      '/?',
      'what does this command do',
      'options',
      'flags',
      'usage',
      'how do i exit man'
    ]
  },
  {
    id: 'sh-bashrc',
    shell: 'bash',
    category: 'shell',
    task: 'Change what runs every time a shell opens',
    summary:
      '~/.bashrc is read by every new bash window: aliases, exports and PATH additions go at its end. After editing, source it to apply the change to the window you are in.',
    command: 'nano ~/.bashrc',
    variants: [
      {
        label: 'Apply ~/.bashrc changes to this shell',
        command: 'source ~/.bashrc',
        keywords: [
          'source',
          'dot command',
          'reload',
          'without reopening',
          'take effect now',
          'apply now',
          'refresh config'
        ]
      }
    ],
    keywords: [
      'bashrc',
      '.bashrc',
      'profile',
      '$profile',
      'startup script',
      'shell config',
      'source',
      'reload shell',
      'bash_profile',
      'settings file',
      'autoexec'
    ]
  },
  {
    id: 'sh-alias',
    shell: 'bash',
    category: 'shell',
    task: 'Make a short name for a long command',
    summary:
      'Lasts until this shell closes; the second variant saves it for every new one. No spaces around the =.',
    command: "alias gs='git status'",
    variants: [
      {
        label: 'List the aliases already set',
        command: 'alias',
        keywords: ['alias', 'show my shortcuts', 'what is defined', 'existing', 'll']
      },
      {
        label: 'Save an alias in ~/.bashrc',
        command: 'echo "alias gs=\'git status\'" >> ~/.bashrc',
        keywords: ['bashrc', 'permanent', 'every new shell', 'survives a restart', 'for good']
      },
      {
        label: 'Remove an alias',
        command: 'unalias SHORT',
        keywords: ['unalias', 'delete a shortcut', 'undo', 'get rid of it', 'stop overriding']
      }
    ],
    placeholders: { SHORT: 'The alias to remove, for example gs' },
    keywords: [
      'alias',
      'shortcut command',
      'set-alias',
      'doskey',
      'abbreviation',
      'custom command',
      'nickname',
      'unalias',
      'll'
    ]
  },
  {
    id: 'sh-redirect',
    shell: 'bash',
    category: 'shell',
    task: "Save a command's output to a file",
    summary:
      'A single > writes a fresh file each time; a double >> adds to the end of what is there.',
    command: 'npm run build > FILE',
    variants: [
      {
        label: 'Append to the end of a file',
        command: 'npm run build >> FILE',
        keywords: ['>>', 'double arrow', 'add to it', 'do not overwrite', 'keep what is there']
      },
      {
        label: 'Save error messages as well',
        command: 'npm run build &> FILE',
        keywords: ['2>&1', '&>', 'stderr', 'capture errors', 'both streams', 'full log']
      },
      {
        label: 'Throw error messages away',
        command: 'npm run build 2>/dev/null',
        keywords: ['2>/dev/null', 'silence', 'hide stderr', 'suppress', 'quiet']
      },
      {
        label: 'Show on screen and save to a file',
        command: 'npm run build | tee FILE',
        keywords: ['tee', 'both at once', 'log and watch', 'split the output', 'copy to a file']
      }
    ],
    placeholders: { FILE: 'The file to write, for example output.txt' },
    keywords: [
      'redirect',
      'redirection',
      'save output',
      'write to file',
      'log to file',
      'out-file',
      'append',
      'stderr',
      '2>&1',
      '/dev/null',
      'tee',
      'hide errors'
    ],
    danger:
      'A single > (and tee) replaces everything the file held before. Use >> (or tee -a) to keep it.'
  },
  {
    id: 'sh-pipe',
    shell: 'bash',
    category: 'shell',
    task: 'Send the output of one command into another',
    summary:
      'The bar hands what the first command prints to the second. less pages it (Q to leave); clip.exe puts it on the Windows clipboard.',
    command: 'ps aux | less',
    variants: [
      {
        label: 'Copy the output to the Windows clipboard',
        command: 'pwd | clip.exe',
        keywords: ['clip.exe', 'paste', 'pbcopy', 'xclip', 'ctrl v', 'send to windows']
      },
      {
        label: 'Use each output line as an argument',
        command: 'find . -name "*.log" | xargs OTHER',
        keywords: ['xargs', 'for each', 'batch', 'apply to every one', 'feed into']
      }
    ],
    placeholders: { OTHER: 'The command that should receive the lines, for example ls -l' },
    keywords: [
      'pipe',
      'pipeline',
      'vertical bar',
      'less',
      'more',
      'page output',
      'clip',
      'copy output to clipboard',
      'pbcopy',
      'xclip',
      'set-clipboard',
      'xargs'
    ]
  },
  {
    id: 'sh-several-commands',
    shell: 'bash',
    category: 'shell',
    task: 'Run several commands on one line',
    summary:
      '&& runs the second command only if the first one worked, which is nearly always what you want.',
    command: 'npm install && SECOND',
    variants: [
      {
        label: 'Run the second command whatever happens',
        command: 'npm test ; SECOND',
        keywords: ['semicolon', 'always', 'regardless', 'separate them', 'ignore failure']
      },
      {
        label: 'Run the second only if the first fails',
        command: 'npm test || SECOND',
        keywords: ['||', 'or operator', 'on failure', 'fallback', 'else']
      }
    ],
    placeholders: { SECOND: 'The command to run after it, for example npm run dev' },
    keywords: [
      'chain commands',
      'multiple commands',
      'one line',
      'and and',
      'semicolon',
      'run after',
      'if it fails',
      'then run',
      'ampersand'
    ]
  },
  {
    id: 'sh-exit',
    shell: 'bash',
    category: 'shell',
    task: 'Leave the shell, or an SSH session',
    summary:
      'Closes the shell you are in: out of an SSH login back to your own machine, or out of WSL back to Windows. Ctrl+D on an empty line does the same.',
    command: 'exit',
    keywords: [
      'exit',
      'quit',
      'logout',
      'log out',
      'close terminal',
      'leave ssh',
      'ctrl d',
      'disconnect',
      'leave wsl',
      'end session'
    ]
  },
  // INSTALLING THE AGENTS, this shell's way (2026-10-13), from the vendors'
  // own pages: code.claude.com/docs/en/setup and github.com/openai/codex.
  {
    id: 'sh-install-claude',
    shell: 'bash',
    category: 'agents',
    task: 'Install Claude Code',
    summary:
      'The native installer, the recommended way: it keeps Claude Code up to date by itself. In WSL, install inside the WSL tab, not from Windows. When it finishes, open a new tab and check claude --version.',
    command: 'curl -fsSL https://claude.ai/install.sh | bash',
    variants: [
      {
        label: 'Install Claude Code with npm',
        command: 'npm install -g @anthropic-ai/claude-code',
        keywords: ['node', 'npm global', 'node 22', 'anthropic-ai/claude-code']
      },
      {
        label: 'Check Claude Code is installed',
        command: 'claude --version',
        keywords: ['did it work', 'which version', 'claude not found', 'verify install']
      }
    ],
    keywords: ['install claude', 'get claude code', 'set up claude', 'claude code installer', 'download claude', 'anthropic cli', 'wsl', 'linux']
  },
  {
    id: 'sh-install-codex',
    shell: 'bash',
    category: 'agents',
    task: 'Install Codex',
    summary:
      "OpenAI's standalone installer for the Codex CLI. In WSL, install inside the WSL tab, not from Windows. When it finishes, open a new tab and check codex --version.",
    command: 'curl -fsSL https://chatgpt.com/codex/install.sh | sh',
    variants: [
      {
        label: 'Install Codex with npm',
        command: 'npm install -g @openai/codex',
        keywords: ['node', 'npm global', 'openai/codex', 'npm i -g']
      },
      {
        label: 'Check Codex is installed',
        command: 'codex --version',
        keywords: ['did it work', 'which version', 'codex not found', 'verify install']
      }
    ],
    keywords: ['install codex', 'get codex', 'set up codex', 'codex installer', 'openai cli', 'download codex', 'wsl', 'linux']
  }
]
