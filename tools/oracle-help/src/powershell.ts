// COPY of PT/core/shared/help/powershell.ts, import specifiers rewritten, nothing else
/**
 * THE HELP CATALOGUE, PowerShell (#12).
 *
 * PowerShell is the app's default shell, so this is the file most people will
 * be searching. It covers the everyday first (where am I, list, read, copy,
 * delete, find text, what is on this port, environment variables and PATH,
 * "access denied", "scripts are disabled") and then the PowerShell answer to
 * each Unix habit: ls, cat, grep, touch, which, tail -f, head, wc, find, df, du,
 * ps, kill, curl, export, alias, man, history, clear. Those Unix words sit in
 * the keywords, so typing "grep" finds Select-String.
 *
 * Every command is written to work in BOTH Windows PowerShell 5.1 (what a stock
 * Windows 11 ships) and PowerShell 7. Where something is 7 only, it is a variant
 * and its label says so: 'cd -', '&&', 'COMMAND &' and Get-Uptime.
 *
 * HOW THESE WERE CHECKED (2026-09-20, Windows 11 26200, pwsh 7.6.6 AND
 * powershell.exe 5.1.26100): every read-only command was RUN in both shells and
 * its output read, and every file command (make, copy, move, rename, delete,
 * zip, unzip, junction, replace in file, Recycle Bin) was run in both inside a
 * scratch folder made for the purpose. Stop-Process and the free-a-port line
 * were run with -WhatIf only. Four things were found that way and shaped the
 * entries: Get-NetTCPConnection answers a free port with a wall of red, so the
 * port entries silence it and say that nothing printed means nothing listening;
 * a junction wants a FULL path in 7 and takes a relative one in 5.1; a symbolic
 * link is refused without an administrator shell; and in 5.1 'curl' and 'wget'
 * are aliases of Invoke-WebRequest, so the real curl is 'curl.exe'. Not run, on
 * purpose: anything that opens a window or a prompt (explorer, Invoke-Item,
 * notepad, Start-Process on a URL, -Verb RunAs, Out-Host -Paging), anything that
 * writes outside the scratch folder (Set-ExecutionPolicy, the permanent
 * environment variable and PATH lines, the profile), Set-Clipboard (it would
 * replace the owner's clipboard), Get-Content -Wait (it never returns) and
 * ipconfig /flushdns.
 */

import type { HelpEntry } from './types.ts'

export const POWERSHELL_HELP: readonly HelpEntry[] = [
  // ------------------------------------------------------------------ files
  {
    id: 'ps-list-files',
    shell: 'powershell',
    category: 'files',
    task: 'List the files in this folder',
    summary:
      'Shows what is in the current folder, with sizes and dates. ls and dir are built-in short names for the same command.',
    command: 'Get-ChildItem',
    variants: [
      {
        label: 'Sort the listing by date, newest first',
        command: 'Get-ChildItem | Sort-Object LastWriteTime -Descending',
        keywords: ['ls -t', 'most recently changed', 'lastwritetime', 'what changed last']
      },
      {
        label: 'Sort the listing by size, biggest first',
        command: 'Get-ChildItem -File | Sort-Object Length -Descending',
        keywords: ['ls -s', 'largest in this folder', 'length descending', 'what is big here']
      },
      {
        label: 'List file names and nothing else',
        command: 'Get-ChildItem -Name',
        keywords: ['bare list', 'ls -1', 'no columns', 'just the names', 'paste a list']
      },
      {
        label: 'List the folders, not the files',
        command: 'Get-ChildItem -Directory',
        keywords: ['ls -d', 'subdirectories', 'what folders are in here', 'directories only']
      },
      {
        label: 'List only files of one extension',
        command: "Get-ChildItem -Filter '*.EXTENSION'",
        keywords: ['ls *.json', 'dir *.txt', 'by extension', 'filter file type', 'wildcard']
      }
    ],
    placeholders: { EXTENSION: 'A file ending, such as txt or json' },
    keywords: [
      'ls',
      'dir',
      'ls -l',
      'ls -la',
      'list everything',
      'list files',
      'show files',
      'list another folder',
      'files here',
      'what files are here',
      'whats in this folder',
      'what is in this folder',
      'directory listing',
      'folder contents',
      'gci',
      'sort by date'
    ]
  },
  {
    id: 'ps-show-hidden-files',
    shell: 'powershell',
    category: 'files',
    task: 'See hidden files and folders',
    summary:
      'Adds hidden and system items to the listing, such as .git and AppData, which a plain listing leaves out.',
    command: 'Get-ChildItem -Force',
    variants: [
      {
        label: 'Show hidden items and nothing else',
        command: 'Get-ChildItem -Hidden',
        keywords: [
          'only dotfiles',
          'hidden attribute',
          'exclude normal files',
          'just the invisible'
        ]
      }
    ],
    keywords: [
      'ls -a',
      'dir /a',
      'hidden files',
      'show hidden',
      'dotfiles',
      '.git folder',
      '.env file',
      'invisible files',
      'cannot see file',
      'appdata'
    ]
  },
  {
    id: 'ps-read-file',
    shell: 'powershell',
    category: 'files',
    task: 'Read a file in the terminal',
    summary:
      'Prints the whole file to the screen. cat and type are short names for the same command.',
    command: 'Get-Content FILE',
    variants: [
      {
        label: 'Read a file a page at a time',
        command: 'Get-Content FILE | Out-Host -Paging',
        keywords: ['less', 'more', 'pager', 'space for the next screen', 'scroll through slowly']
      }
    ],
    placeholders: { FILE: 'The file to read' },
    keywords: [
      'cat',
      'type',
      'print file',
      'show file contents',
      'view file',
      'display file',
      'open text file',
      'read text',
      'less',
      'more',
      'gc'
    ]
  },
  {
    id: 'ps-first-lines',
    shell: 'powershell',
    category: 'files',
    task: 'See the first lines of a file',
    summary: 'Prints only the top of a file, which is what you want for a big log or a CSV header.',
    command: 'Get-Content -TotalCount 20 FILE',
    variants: [
      {
        label: 'Read a range of lines from a file',
        command: 'Get-Content FILE | Select-Object -Skip 10 -First 5',
        keywords: ['sed -n', 'lines 11 to 15', 'skip then take', 'middle of the file', 'line range']
      },
      {
        label: 'Keep the first rows of any output',
        command: 'Get-Process | Select-Object -First 10',
        keywords: ['| head', 'limit results', 'first 10 results', 'shorten a long list']
      }
    ],
    placeholders: { FILE: 'The file to read' },
    keywords: [
      'head',
      'head -n',
      'top of file',
      'first 10 lines',
      'beginning of file',
      'start of file',
      'preview file',
      'peek'
    ]
  },
  {
    id: 'ps-last-lines',
    shell: 'powershell',
    category: 'files',
    task: 'See the last lines of a file',
    summary: 'Prints only the end of a file, where a log keeps its newest entries.',
    command: 'Get-Content -Tail 20 FILE',
    variants: [
      {
        label: 'Keep the last rows of any output',
        command: 'Get-ChildItem | Select-Object -Last 10',
        keywords: ['| tail', 'end of a long list', 'final results', 'bottom of the output']
      }
    ],
    placeholders: { FILE: 'The file to read' },
    keywords: [
      'tail',
      'tail -n',
      'end of file',
      'last 10 lines',
      'bottom of file',
      'latest log lines',
      'recent log entries'
    ]
  },
  {
    id: 'ps-follow-file',
    shell: 'powershell',
    category: 'files',
    task: 'Watch a log file as it grows',
    summary:
      'Prints the last lines and then keeps printing new ones as they are written. Press Ctrl+C to stop watching.',
    command: 'Get-Content -Tail 20 -Wait FILE',
    variants: [
      {
        label: 'Follow a log, filtered to one word',
        command: "Get-Content FILE -Tail 20 -Wait | Select-String 'TEXT'",
        keywords: ['tail -f | grep', 'live errors only', 'watch for a word', 'filter a live log']
      }
    ],
    placeholders: { FILE: 'The log file to follow', TEXT: 'The word to look for, such as error' },
    keywords: [
      'tail -f',
      'follow log',
      'watch file',
      'live log',
      'stream log',
      'monitor file',
      'log keeps updating',
      'real time log'
    ]
  },
  {
    id: 'ps-make-file',
    shell: 'powershell',
    category: 'files',
    task: 'Make a new empty file',
    summary:
      'Creates an empty file and refuses if one of that name already exists, so it can never wipe anything.',
    command: 'New-Item -ItemType File FILE',
    variants: [
      {
        label: 'Update a file timestamp to now',
        command: '(Get-Item FILE).LastWriteTime = Get-Date',
        keywords: ['touch', 'lastwritetime', 'modified date', 'bump mtime', 'mark as changed']
      }
    ],
    placeholders: { FILE: 'The name of the new file, such as notes.txt' },
    keywords: [
      'touch',
      'create file',
      'new file',
      'empty file',
      'blank file',
      'make a file',
      'ni',
      'type nul',
      'add file'
    ]
  },
  {
    id: 'ps-write-text-to-file',
    shell: 'powershell',
    category: 'files',
    task: 'Put a line of text into a file',
    summary:
      'Writes the text as the whole content of the file, creating it if needed. Use the Add-Content variant to keep what is there.',
    command: "Set-Content FILE 'TEXT'",
    variants: [
      {
        label: 'Append a line to a file',
        command: "Add-Content FILE 'TEXT'",
        keywords: ['echo >>', 'add-content', 'without wiping it', 'add to the end', 'log a line']
      }
    ],
    placeholders: { FILE: 'The file to write', TEXT: 'The text to put in it' },
    keywords: [
      'echo to file',
      'echo >',
      'echo >>',
      'write file',
      'append to file',
      'add line to file',
      'create file with text',
      'save text',
      '.gitignore',
      '.env'
    ],
    danger: 'Set-Content replaces everything already in the file.'
  },
  {
    id: 'ps-copy-file',
    shell: 'powershell',
    category: 'files',
    task: 'Copy a file',
    summary:
      'Copies a file to a new name or into another folder. cp and copy are short names for it.',
    command: 'Copy-Item SOURCE DESTINATION',
    variants: [
      {
        label: 'Copy every file of one extension',
        command: "Copy-Item '*.EXTENSION' FOLDER",
        keywords: ['cp *.txt', 'wildcard', 'bulk copy', 'all the jpgs', 'many files at once']
      }
    ],
    placeholders: {
      SOURCE: 'The file to copy',
      DESTINATION: 'The new name, or the folder to copy it into',
      EXTENSION: 'A file ending, such as txt',
      FOLDER: 'The folder to copy into'
    },
    keywords: ['cp', 'copy', 'duplicate file', 'make a copy', 'backup file', 'clone file', 'cpi'],
    danger: 'A file of the same name at the destination is replaced without a question.'
  },
  {
    id: 'ps-move-file',
    shell: 'powershell',
    category: 'files',
    task: 'Move a file or folder somewhere else',
    summary:
      'Moves a file or a whole folder. It stops with an error rather than replacing something of the same name at the destination.',
    command: 'Move-Item SOURCE DESTINATION',
    placeholders: {
      SOURCE: 'The file or folder to move',
      DESTINATION: 'The folder to move it into, or its new path'
    },
    keywords: [
      'mv',
      'move',
      'relocate',
      'put file in folder',
      'cut and paste',
      'transfer file',
      'mi'
    ]
  },
  {
    id: 'ps-rename-file',
    shell: 'powershell',
    category: 'files',
    task: 'Rename a file or folder',
    summary: 'Gives a file or folder a new name where it sits.',
    command: 'Rename-Item OLD_NAME NEW_NAME',
    placeholders: { OLD_NAME: 'The name it has now', NEW_NAME: 'The name it should have' },
    keywords: ['ren', 'rename', 'mv', 'change file name', 'change extension', 'new name', 'rni']
  },
  {
    id: 'ps-delete-file',
    shell: 'powershell',
    category: 'files',
    task: 'Delete a file',
    summary:
      'Removes the file straight away. It does NOT go to the Recycle Bin, so use the Recycle Bin variant when you might want it back.',
    command: 'Remove-Item FILE',
    variants: [
      {
        label: 'Preview a delete without deleting',
        command: 'Remove-Item -WhatIf FILE',
        keywords: ['-whatif', 'dry run', 'simulate', 'rm -i', 'is it safe', 'what would go']
      },
      {
        label: 'Delete a file to the Recycle Bin',
        command:
          "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile((Resolve-Path FILE), 'OnlyErrorDialogs', 'SendToRecycleBin')",
        keywords: ['trash', 'recoverable', 'get it back later', 'undo a delete', 'safe removal']
      },
      {
        label: 'Delete every file of one extension',
        command: "Remove-Item '*.EXTENSION'",
        keywords: ['del *.log', 'wildcard', 'clear out the logs', 'bulk removal', 'many at once']
      }
    ],
    placeholders: { FILE: 'The file to delete', EXTENSION: 'A file ending, such as log' },
    keywords: [
      'rm',
      'del',
      'erase',
      'delete file',
      'remove file',
      'get rid of file',
      'recycle bin',
      'trash',
      'unlink',
      'ri'
    ],
    danger: 'The file is gone for good: Remove-Item does not use the Recycle Bin.'
  },
  {
    id: 'ps-find-file-by-name',
    shell: 'powershell',
    category: 'files',
    task: 'Find a file by name, in any subfolder',
    summary:
      'Searches this folder and everything under it for names containing the word. Folders it may not read are skipped quietly.',
    command: "Get-ChildItem -Recurse -Filter '*NAME*' -ErrorAction SilentlyContinue",
    variants: [
      {
        label: 'Find files by name, as full paths',
        command: "(Get-ChildItem -Recurse -Filter '*NAME*' -ErrorAction SilentlyContinue).FullName",
        keywords: ['fullname', 'absolute path results', 'paths only', 'copyable list', 'no table']
      },
      {
        label: 'Find every file of one extension',
        command: "Get-ChildItem -Recurse -File -Filter '*.EXTENSION' -ErrorAction SilentlyContinue",
        keywords: ['find . -name "*.pdf"', 'all the pdfs', 'by file type', 'where are my mp4s']
      },
      {
        label: 'Find a folder by its name',
        command: "Get-ChildItem -Recurse -Directory -Filter '*NAME*' -ErrorAction SilentlyContinue",
        keywords: ['find -type d', 'locate a directory', 'which folder is it in', 'dirs only']
      },
      {
        label: 'Search from a different folder',
        command: "Get-ChildItem FOLDER -Recurse -Filter '*NAME*' -ErrorAction SilentlyContinue",
        keywords: ['find /path -name', 'look somewhere else', 'start point', 'search c drive']
      }
    ],
    placeholders: {
      NAME: 'Part of the file name',
      EXTENSION: 'A file ending, such as pdf',
      FOLDER: 'Where to start looking'
    },
    keywords: [
      'find',
      'find . -name',
      'dir /s',
      'locate',
      'search for file',
      'where is my file',
      'lost file',
      'look for file',
      'recursive search',
      'file name search'
    ]
  },
  {
    id: 'ps-biggest-files',
    shell: 'powershell',
    category: 'files',
    task: 'Find the biggest files in a folder',
    summary:
      'Lists the twenty largest files under the current folder with their size in megabytes: the first thing to run when a disk is full.',
    command:
      "Get-ChildItem -Recurse -File -ErrorAction SilentlyContinue | Sort-Object Length -Descending | Select-Object -First 20 FullName, @{n='MB';e={[math]::Round($_.Length/1MB,1)}}",
    variants: [
      {
        label: 'List files over 100 MB',
        command:
          'Get-ChildItem -Recurse -File -ErrorAction SilentlyContinue | Where-Object Length -gt 100MB | Sort-Object Length -Descending | Select-Object FullName, Length',
        keywords: ['find -size +100m', 'huge files', 'above a threshold', 'gigabyte files']
      }
    ],
    keywords: [
      'big files',
      'large files',
      'largest files',
      'what is taking up space',
      'disk full',
      'free up space',
      'du',
      'find -size',
      'huge files',
      'out of disk space',
      'sort by size'
    ]
  },
  {
    id: 'ps-recent-files',
    shell: 'powershell',
    category: 'files',
    task: 'Find files changed recently',
    summary:
      'Lists the files under this folder that were modified in the last day, newest first. Handy for seeing what an agent or an installer just touched.',
    command:
      'Get-ChildItem -Recurse -File | Where-Object LastWriteTime -gt (Get-Date).AddDays(-1) | Sort-Object LastWriteTime -Descending | Select-Object LastWriteTime, FullName',
    variants: [
      {
        label: 'Files changed in the last hour',
        command:
          'Get-ChildItem -Recurse -File | Where-Object LastWriteTime -gt (Get-Date).AddHours(-1) | Sort-Object LastWriteTime -Descending | Select-Object LastWriteTime, FullName',
        keywords: ['find -mmin -60', 'just touched', 'last 60 minutes', 'what did it write']
      }
    ],
    keywords: [
      'find -mtime',
      'recently modified',
      'changed today',
      'what changed',
      'new files',
      'latest files',
      'newest files',
      'modified in the last',
      'what did it touch',
      'files edited'
    ]
  },
  {
    id: 'ps-search-in-files',
    shell: 'powershell',
    category: 'files',
    task: 'Find text inside the files of a folder',
    summary:
      'Searches every file under this folder and prints file, line number and the matching line. The text is a regular expression and case does not matter.',
    command: "Get-ChildItem -Recurse -File | Select-String 'TEXT'",
    variants: [
      {
        label: 'Search inside one kind of file only',
        command: "Get-ChildItem -Recurse -File -Include *.ts, *.js | Select-String 'TEXT'",
        keywords: ['--include', 'only ts and js', 'by extension', 'source files', 'narrow it down']
      },
      {
        label: 'Search the code but skip node_modules',
        command:
          "Get-ChildItem -Recurse -File | Where-Object FullName -notlike '*\\node_modules\\*' | Select-String 'TEXT'",
        keywords: ['--exclude-dir', 'ignore vendor folders', 'too many results', 'without deps']
      },
      {
        label: 'List the files that contain the text',
        command:
          "Get-ChildItem -Recurse -File | Select-String 'TEXT' -List | Select-Object -ExpandProperty Path",
        keywords: ['grep -l', 'which file holds it', 'names only', 'one hit per file', 'paths']
      },
      {
        label: 'Search for exact text, not a pattern',
        command: "Get-ChildItem -Recurse -File | Select-String -SimpleMatch 'TEXT'",
        keywords: [
          '-simplematch',
          'literal string',
          'no regex',
          'brackets and dots',
          'fixed string'
        ]
      }
    ],
    placeholders: { TEXT: 'The text to look for' },
    keywords: [
      'grep',
      'grep -r',
      'grep -rn',
      'rg',
      'ripgrep',
      'findstr',
      'findstr /s',
      'search in files',
      'search all files for a word',
      'find text in files',
      'which file contains',
      'search code',
      'find in folder',
      'sls'
    ]
  },
  {
    id: 'ps-zip',
    shell: 'powershell',
    category: 'files',
    task: 'Zip a folder or some files',
    summary:
      'Packs a folder (or files) into a .zip. It refuses to replace a zip that already exists.',
    command: 'Compress-Archive -Path FOLDER -DestinationPath NAME.zip',
    variants: [
      {
        label: 'Update a zip that already exists',
        command: 'Compress-Archive -Path FOLDER -DestinationPath NAME.zip -Update',
        keywords: ['-update', 'add to an archive', 'refresh the contents', 'already exists error']
      },
      {
        label: 'Make a .tar.gz archive',
        command: 'tar -czf NAME.tar.gz FOLDER',
        keywords: ['tar -czf', 'tarball', 'gzip', 'tgz', 'linux friendly archive']
      }
    ],
    placeholders: { FOLDER: 'The folder or file to pack', NAME: 'The name of the archive to make' },
    keywords: [
      'zip',
      'compress',
      'archive',
      'make a zip',
      'pack folder',
      'tar',
      'tar -czf',
      'create archive',
      'send folder',
      'shrink'
    ]
  },
  {
    id: 'ps-unzip',
    shell: 'powershell',
    category: 'files',
    task: 'Unzip an archive',
    summary:
      'Unpacks a .zip into a folder, making the folder if needed. It stops if a file is already there, unless you add -Force.',
    command: 'Expand-Archive NAME -DestinationPath FOLDER',
    variants: [
      {
        label: 'List what is inside an archive',
        command: 'tar -tf NAME',
        keywords: ['tar -tf', 'peek in a zip', 'contents without extracting', 'what is in it']
      },
      {
        label: 'Unpack a .tar.gz or .tgz here',
        command: 'tar -xzf NAME',
        keywords: ['tar -xzf', 'untar', 'gunzip', 'open a tarball', 'linux archive']
      }
    ],
    placeholders: {
      NAME: 'The archive to unpack, such as photos.zip',
      FOLDER: 'Where the contents should go'
    },
    keywords: [
      'unzip',
      'extract',
      'unpack',
      'decompress',
      'open zip',
      'tar -xzf',
      'untar',
      'expand archive',
      'extract zip',
      'tgz'
    ]
  },
  {
    id: 'ps-file-hash',
    shell: 'powershell',
    category: 'files',
    task: 'Check the checksum of a download',
    summary:
      'Prints the SHA-256 hash of a file, to compare with the one a download page publishes. If they differ, the file is damaged or not the real one.',
    command: 'Get-FileHash FILE',
    variants: [
      {
        label: 'Get the MD5 hash of a file',
        command: 'Get-FileHash -Algorithm MD5 FILE',
        keywords: ['md5sum', 'certutil md5', 'compare checksums', 'older download pages']
      },
      {
        label: 'Get the SHA-1 hash of a file',
        command: 'Get-FileHash -Algorithm SHA1 FILE',
        keywords: ['sha1sum', 'shasum -a 1', 'git style hash', 'verify a signature']
      }
    ],
    placeholders: { FILE: 'The file to check' },
    keywords: [
      'sha256sum',
      'md5sum',
      'shasum',
      'checksum',
      'hash',
      'verify download',
      'integrity',
      'certutil -hashfile',
      'file fingerprint',
      'is this file the same'
    ]
  },
  {
    id: 'ps-open-file',
    shell: 'powershell',
    category: 'files',
    task: 'Open a file in its usual app',
    summary:
      'Opens the file the way a double-click in Explorer would: a PDF in the PDF reader, a picture in the photo viewer.',
    command: 'Invoke-Item FILE',
    variants: [
      {
        label: 'Open a file in Notepad',
        command: 'notepad FILE',
        keywords: ['text editor', 'edit a txt', 'quick edit', 'gui editor', 'open in a window']
      },
      {
        label: 'Open a file in VS Code',
        command: 'code FILE',
        keywords: ['vscode', 'code .', 'editor', 'ide', 'open the project']
      }
    ],
    placeholders: { FILE: 'The file to open' },
    keywords: [
      'open',
      'xdg-open',
      'start',
      'double click',
      'launch file',
      'default app',
      'open with',
      'ii',
      'edit file',
      'open in editor'
    ]
  },
  {
    id: 'ps-file-info',
    shell: 'powershell',
    category: 'files',
    task: 'See the size and dates of a file',
    summary: 'Shows how big a file is in bytes, when it was made and when it last changed.',
    command: 'Get-Item FILE | Select-Object Name, Length, CreationTime, LastWriteTime, Attributes',
    variants: [
      {
        label: 'Show every property of a file',
        command: 'Get-Item FILE | Format-List *',
        keywords: ['format-list', 'all the metadata', 'full details', 'every field', 'stat -x']
      }
    ],
    placeholders: { FILE: 'The file to look at' },
    keywords: [
      'stat',
      'file size',
      'file details',
      'properties',
      'modified date',
      'created date',
      'how big is this file',
      'when was it changed',
      'read only',
      'attributes'
    ]
  },
  {
    id: 'ps-full-path',
    shell: 'powershell',
    category: 'files',
    task: 'Get the full path of a file',
    summary:
      'Turns a short or relative name into the complete path from the drive letter, ready to paste into another app or an agent.',
    command: '(Resolve-Path FILE).Path',
    variants: [
      {
        label: 'Copy the full path to the clipboard',
        command: '(Resolve-Path FILE).Path | Set-Clipboard',
        keywords: ['pbcopy', 'set-clipboard', 'paste into chat', 'give a path to an agent']
      },
      {
        label: 'Check whether a file or folder exists',
        command: 'Test-Path FILE',
        keywords: ['test -f', 'if exist', 'true or false', 'is it there', 'missing file']
      }
    ],
    placeholders: { FILE: 'The file or folder' },
    keywords: [
      'realpath',
      'readlink -f',
      'absolute path',
      'full path',
      'copy path',
      'path of file',
      'complete path',
      'does file exist',
      'test -f',
      'if exist'
    ]
  },
  {
    id: 'ps-make-link',
    shell: 'powershell',
    category: 'files',
    task: 'Link a folder into a second place',
    summary:
      'Creates a junction: a folder that is really another folder. It needs no administrator rights, and the target must be written as a full path.',
    command: 'New-Item -ItemType Junction -Path LINK -Target FULL_PATH',
    variants: [
      {
        label: 'Make a symbolic link (needs admin)',
        command: 'New-Item -ItemType SymbolicLink -Path LINK -Target FULL_PATH',
        keywords: ['ln -s', 'mklink', 'symlink a file', 'elevated shell', 'shortcut to a file']
      }
    ],
    placeholders: {
      LINK: 'The name of the new link',
      FULL_PATH: 'The full path of the real folder, from the drive letter'
    },
    keywords: [
      'ln -s',
      'symlink',
      'symbolic link',
      'mklink',
      'junction',
      'shortcut folder',
      'link folder',
      'alias folder',
      'administrator privilege required'
    ]
  },

  // ---------------------------------------------------------------- folders
  {
    id: 'ps-where-am-i',
    shell: 'powershell',
    category: 'folders',
    task: 'See which folder I am in',
    summary:
      'Prints the current folder, the one every command without a path acts on. pwd is a short name for it.',
    command: 'Get-Location',
    variants: [
      {
        label: 'Copy the current path to the clipboard',
        command: '(Get-Location).Path | Set-Clipboard',
        keywords: ['pwd | pbcopy', 'set-clipboard', 'paste where i am', 'send a path to someone']
      }
    ],
    keywords: [
      'pwd',
      'where am i',
      'current folder',
      'current directory',
      'working directory',
      'cd',
      'which folder',
      'path of this folder',
      'gl'
    ]
  },
  {
    id: 'ps-change-folder',
    shell: 'powershell',
    category: 'folders',
    task: 'Go to another folder',
    summary:
      'Moves the shell into a folder. cd is a short name for it. Put quotes round a path that has spaces, and press Tab to complete a name.',
    command: 'Set-Location FOLDER',
    variants: [
      {
        label: 'Go to a path that has spaces',
        command: "Set-Location 'C:\\Program Files'",
        keywords: ['quotes', 'program files', 'space in the name', 'quoting a path']
      },
      {
        label: 'Go to your home folder',
        command: 'Set-Location ~',
        keywords: ['cd ~', 'userprofile', 'c:\\users', 'my user directory']
      },
      {
        label: 'Go to your Downloads folder',
        command: 'Set-Location ~\\Downloads',
        keywords: ['cd ~/downloads', 'where a browser saves', 'the installer i just got']
      },
      {
        label: 'Switch to another drive',
        command: 'Set-Location D:',
        keywords: ['d:', 'drive letter', 'usb stick', 'second disk', 'external drive']
      },
      {
        label: 'Return to the previous folder (pwsh 7)',
        command: 'Set-Location -',
        keywords: ['cd -', 'toggle', 'where i just was', 'jump straight back', 'powershell 7']
      }
    ],
    placeholders: { FOLDER: 'The folder to go to' },
    keywords: [
      'cd',
      'chdir',
      'change directory',
      'go to folder',
      'navigate',
      'enter folder',
      'switch drive',
      'home folder',
      'cd ~',
      'cd -',
      'path with spaces',
      'sl'
    ]
  },
  {
    id: 'ps-go-up-a-folder',
    shell: 'powershell',
    category: 'folders',
    task: 'Go back up a folder',
    summary: 'Two dots mean "the folder that holds this one", so this steps out one level.',
    command: 'Set-Location ..',
    variants: [
      {
        label: 'Go up two folder levels',
        command: 'Set-Location ..\\..',
        keywords: ['cd ../..', 'grandparent', 'out twice', 'two levels']
      },
      {
        label: 'Go straight to the top of the drive',
        command: 'Set-Location \\',
        keywords: ['cd /', 'root of c', 'drive root', 'top level']
      }
    ],
    keywords: [
      'cd ..',
      'go back',
      'parent folder',
      'up one level',
      'previous folder',
      'back a folder',
      'leave folder',
      'go up',
      'out of this folder'
    ]
  },
  {
    id: 'ps-make-folder',
    shell: 'powershell',
    category: 'folders',
    task: 'Make a new folder',
    summary:
      'Creates a folder, and any missing folders on the way to it, so A\\B\\C works in one go. mkdir is a short name for it.',
    command: 'New-Item -ItemType Directory FOLDER',
    variants: [
      {
        label: 'Make a folder with mkdir',
        command: 'mkdir FOLDER',
        keywords: ['mkdir', 'md', 'mkdir -p', 'md newfolder', 'the linux name for it']
      }
    ],
    placeholders: { FOLDER: 'The name of the new folder' },
    keywords: [
      'mkdir',
      'mkdir -p',
      'md',
      'create folder',
      'new folder',
      'make directory',
      'create directory',
      'nested folders',
      'add folder'
    ]
  },
  {
    id: 'ps-delete-folder',
    shell: 'powershell',
    category: 'folders',
    task: 'Delete a folder and everything in it',
    summary:
      'Removes the folder with all its files and subfolders. Run the -WhatIf variant first to read the list of what would go.',
    command: 'Remove-Item -Recurse FOLDER',
    variants: [
      {
        label: 'Preview a folder delete, without deleting',
        command: 'Remove-Item -Recurse -WhatIf FOLDER',
        keywords: ['-whatif', 'dry run', 'simulate rm -rf', 'what would go', 'is it safe']
      },
      {
        label: 'Delete a folder including read-only files',
        command: 'Remove-Item -Recurse -Force FOLDER',
        keywords: ['rm -rf', '-force', 'node_modules', '.git', 'access denied', 'it will not go']
      }
    ],
    placeholders: { FOLDER: 'The folder to delete' },
    keywords: [
      'rm -rf',
      'rmdir',
      'rd /s',
      'delete folder',
      'remove directory',
      'delete node_modules',
      'wipe folder',
      'directory not empty',
      'remove folder'
    ],
    danger: 'Everything inside the folder is deleted for good, with no Recycle Bin.'
  },
  {
    id: 'ps-copy-folder',
    shell: 'powershell',
    category: 'folders',
    task: 'Copy a whole folder',
    summary:
      'Copies a folder with everything in it. If DESTINATION already exists the copy lands INSIDE it, otherwise DESTINATION becomes the copy.',
    command: 'Copy-Item FOLDER DESTINATION -Recurse',
    variants: [
      {
        label: 'Copy a big folder with robocopy',
        command: 'robocopy FOLDER DESTINATION /E',
        keywords: ['robocopy /e', 'xcopy', 'rsync', 'progress and retries', 'huge transfer']
      }
    ],
    placeholders: { FOLDER: 'The folder to copy', DESTINATION: 'Where the copy goes' },
    keywords: [
      'cp -r',
      'xcopy',
      'robocopy',
      'copy directory',
      'duplicate folder',
      'backup folder',
      'copy everything',
      'rsync',
      'clone folder'
    ],
    danger: 'Files of the same name already at the destination are replaced.'
  },
  {
    id: 'ps-folder-size',
    shell: 'powershell',
    category: 'folders',
    task: 'See how big a folder is',
    summary:
      'Adds up every file under the folder, hidden ones included, and prints the total in megabytes. A very large folder takes a moment.',
    command:
      "'{0:N1} MB' -f ((Get-ChildItem FOLDER -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum / 1MB)",
    variants: [
      {
        label: 'Size of each subfolder, biggest first',
        command:
          'Get-ChildItem -Directory | ForEach-Object { [pscustomobject]@{ Folder = $_.Name; MB = [math]::Round((Get-ChildItem $_.FullName -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum / 1MB, 1) } } | Sort-Object MB -Descending',
        keywords: ['du -sh *', 'ncdu', 'which one is huge', 'disk usage per directory', 'breakdown']
      }
    ],
    placeholders: { FOLDER: 'The folder to measure, or . for this one' },
    keywords: [
      'du',
      'du -sh',
      'folder size',
      'directory size',
      'how big is this folder',
      'size of node_modules',
      'disk usage',
      'what is taking up space',
      'total size'
    ]
  },
  {
    id: 'ps-folder-tree',
    shell: 'powershell',
    category: 'folders',
    task: 'See a folder as a tree',
    summary:
      'Draws the folders and files under this one as an indented tree. Avoid running it on a folder holding node_modules: the output runs for minutes.',
    command: 'tree /F',
    variants: [
      {
        label: 'Draw the tree of folders, without files',
        command: 'tree',
        keywords: ['tree', 'structure only', 'no files', 'shape of the project']
      },
      {
        label: 'Tree of another folder, in plain ASCII',
        command: 'tree /F /A FOLDER',
        keywords: ['tree /a', 'paste friendly', 'no box characters', 'share the layout']
      },
      {
        label: 'List two levels deep, no drawing',
        command: 'Get-ChildItem -Recurse -Depth 1 -Name',
        keywords: ['-depth 1', 'shallow', 'limited recursion', 'top two levels', 'plain list']
      }
    ],
    placeholders: { FOLDER: 'The folder to draw' },
    keywords: [
      'tree',
      'folder structure',
      'project structure',
      'directory tree',
      'show hierarchy',
      'ls -r',
      'nested folders',
      'outline of folder',
      'layout'
    ]
  },
  {
    id: 'ps-open-in-explorer',
    shell: 'powershell',
    category: 'folders',
    task: 'Open this folder in File Explorer',
    summary: 'Opens a File Explorer window on the current folder. The dot means "here".',
    command: 'explorer .',
    variants: [
      {
        label: 'Open another folder in File Explorer',
        command: 'Invoke-Item FOLDER',
        keywords: ['invoke-item', 'browse elsewhere', 'window on a path', 'gui']
      },
      {
        label: 'Open Explorer with one file selected',
        command: 'explorer "/select,$(Resolve-Path FILE)"',
        keywords: ['/select', 'reveal in explorer', 'highlight the file', 'show me where it is']
      }
    ],
    placeholders: { FOLDER: 'The folder to open', FILE: 'The file to highlight' },
    keywords: [
      'explorer',
      'open folder',
      'file explorer',
      'finder',
      'open .',
      'xdg-open',
      'show in explorer',
      'reveal in explorer',
      'windows explorer',
      'start .'
    ]
  },
  {
    id: 'ps-count-files',
    shell: 'powershell',
    category: 'folders',
    task: 'Count the files in a folder',
    summary: 'Counts every file under this folder, subfolders included.',
    command: '(Get-ChildItem -Recurse -File | Measure-Object).Count',
    variants: [
      {
        label: 'Count files here, ignoring subfolders',
        command: '(Get-ChildItem -File | Measure-Object).Count',
        keywords: ['ls | wc -l', 'top level only', 'no recursion', 'shallow count']
      },
      {
        label: 'Count the files by extension',
        command:
          'Get-ChildItem -Recurse -File | Group-Object Extension | Sort-Object Count -Descending | Select-Object Count, Name',
        keywords: ['group-object', 'how many png', 'breakdown by type', 'what is in here']
      }
    ],
    keywords: [
      'ls | wc -l',
      'how many files',
      'count files',
      'number of files',
      'file count',
      'count by extension',
      'how many items',
      'find | wc -l'
    ]
  },

  // ------------------------------------------------------------------- text
  {
    id: 'ps-filter-output',
    shell: 'powershell',
    category: 'text',
    task: "Filter a command's output down to matching lines",
    summary:
      'Filters what a command prints down to the lines containing the text. Out-String -Stream is what makes it work for PowerShell commands as well as for programs such as npm or git.',
    command: "Get-Process | Out-String -Stream | Select-String 'TEXT'",
    variants: [
      {
        label: 'Hide the lines that mention a word',
        command: "Get-Service | Out-String -Stream | Select-String -NotMatch 'TEXT'",
        keywords: ['grep -v', 'exclude', 'notmatch', 'remove noise', 'everything except']
      },
      {
        label: 'Filter output, upper and lower case exactly',
        command: "Get-ChildItem | Out-String -Stream | Select-String -CaseSensitive 'TEXT'",
        keywords: ['case sensitive', '-casesensitive', 'match capitals', 'exact letters']
      }
    ],
    placeholders: { TEXT: 'The text to keep' },
    keywords: [
      '| grep',
      'grep -v',
      'grep -i',
      'findstr',
      'filter output',
      'search output',
      'only lines containing',
      'pipe to grep',
      'too much output',
      'sls'
    ]
  },
  {
    id: 'ps-search-in-one-file',
    shell: 'powershell',
    category: 'text',
    task: 'Find a word in one file',
    summary:
      'Prints each matching line of the file with its line number. The pattern is a regular expression and ignores case.',
    command: "Select-String -Path FILE -Pattern 'TEXT'",
    variants: [
      {
        label: 'Show two lines of context either side',
        command: "Select-String -Path FILE -Pattern 'TEXT' -Context 2",
        keywords: ['-c 2', 'context lines', 'lines around a match', 'what came before']
      },
      {
        label: 'Match exact text, not a pattern',
        command: "Select-String -Path FILE -Pattern 'TEXT' -SimpleMatch",
        keywords: ['-simplematch', 'literal', 'no regex', 'dots and brackets', 'fixed string']
      },
      {
        label: 'Check every log file in this folder',
        command: "Select-String -Path *.log -Pattern 'TEXT'",
        keywords: ['*.log', 'wildcard path', 'many logs at once', 'which log has the error']
      },
      {
        label: 'Count how many lines match',
        command: "(Select-String -Path FILE -Pattern 'TEXT').Count",
        keywords: ['-c', 'number of hits', 'how often', 'occurrences', 'tally']
      }
    ],
    placeholders: { FILE: 'The file to search', TEXT: 'The text to look for' },
    keywords: [
      'grep one file',
      'grep -n',
      'grep -c',
      'grep context lines',
      'findstr',
      'search file',
      'find word in file',
      'find error in log',
      'line number',
      'regex search'
    ]
  },
  {
    id: 'ps-count-lines',
    shell: 'powershell',
    category: 'text',
    task: 'Count the lines in a file',
    summary: 'Prints how many lines the file has.',
    command: '(Get-Content FILE | Measure-Object -Line).Lines',
    variants: [
      {
        label: 'Count lines, words and characters',
        command: 'Get-Content FILE | Measure-Object -Line -Word -Character',
        keywords: ['wc', 'wc -w', 'how long is this document', 'essay length', 'measure-object']
      },
      {
        label: 'Count the rows any command prints',
        command: '(Get-Process | Measure-Object).Count',
        keywords: ['| wc -l', 'how many results', 'number of items', 'size of a list']
      }
    ],
    placeholders: { FILE: 'The file to count' },
    keywords: [
      'wc',
      'wc -l',
      'wc -w',
      'line count',
      'word count',
      'how many lines',
      'count rows',
      'character count',
      'length of file'
    ]
  },
  {
    id: 'ps-replace-text-in-file',
    shell: 'powershell',
    category: 'text',
    task: 'Replace a word everywhere in a file',
    summary:
      'Reads the file, swaps every match and writes it back. OLD is a regular expression, so put a backslash before characters such as . ( ) or [.',
    command: "(Get-Content FILE -Raw) -replace 'OLD', 'NEW' | Set-Content FILE -NoNewline",
    variants: [
      {
        label: 'Preview a replacement, changing nothing',
        command: "(Get-Content FILE -Raw) -replace 'OLD', 'NEW'",
        keywords: ['dry run', 'check before writing', 'print the result', 'sed without -i']
      }
    ],
    placeholders: {
      FILE: 'The file to change',
      OLD: 'The text to find',
      NEW: 'The text to put in its place'
    },
    keywords: [
      'sed',
      'sed -i',
      'find and replace',
      'search and replace',
      'substitute',
      'swap text',
      'change word in file',
      'replace string',
      'regex replace'
    ],
    danger: 'The file is rewritten in place and there is no undo, so copy it first if it matters.'
  },
  {
    id: 'ps-compare-files',
    shell: 'powershell',
    category: 'text',
    task: 'See what differs between two files',
    summary:
      'Lists the lines that are in one file and not the other: => means only in the second file, <= only in the first. No output means they match.',
    command: 'Compare-Object (Get-Content FILE_A) (Get-Content FILE_B)',
    variants: [
      {
        label: 'Compare two files with fc.exe',
        command: 'fc.exe FILE_A FILE_B',
        keywords: ['fc', 'classic windows tool', 'plain comparison', 'line by line']
      },
      {
        label: 'Coloured diff of two files, using git',
        command: 'git diff --no-index FILE_A FILE_B',
        keywords: ['git diff --no-index', 'proper diff', 'red and green', 'readable comparison']
      }
    ],
    placeholders: { FILE_A: 'The first file', FILE_B: 'The second file' },
    keywords: [
      'diff',
      'compare',
      'fc',
      'difference between files',
      'what changed',
      'are these files the same',
      'side by side',
      'compare two files'
    ]
  },
  {
    id: 'ps-sort-unique-lines',
    shell: 'powershell',
    category: 'text',
    task: 'Sort lines and drop the duplicates',
    summary:
      'Prints the lines of a file in order with each one appearing once. Upper and lower case count as the same.',
    command: 'Get-Content FILE | Sort-Object -Unique',
    variants: [
      {
        label: 'How often each line appears, most first',
        command:
          'Get-Content FILE | Group-Object | Sort-Object Count -Descending | Select-Object Count, Name',
        keywords: ['uniq -c', 'sort | uniq -c | sort -rn', 'frequency', 'top offenders', 'tally']
      }
    ],
    placeholders: { FILE: 'The file to read' },
    keywords: [
      'sort',
      'uniq',
      'sort -u',
      'uniq -c',
      'sort | uniq',
      'remove duplicates',
      'dedupe',
      'unique lines',
      'most common',
      'count occurrences'
    ]
  },
  {
    id: 'ps-pick-columns',
    shell: 'powershell',
    category: 'text',
    task: 'Show only some columns of a result',
    summary:
      'PowerShell commands return rows with named properties, so you pick columns by name rather than cutting text apart.',
    command: 'Get-ChildItem | Select-Object Name, PROPERTY',
    variants: [
      {
        label: 'Show the name and ID of each process',
        command: 'Get-Process | Select-Object Name, Id',
        keywords: ['process name and pid', 'pid column', 'two columns', 'trim a wide table']
      },
      {
        label: 'Find out which properties a result has',
        command: 'Get-ChildItem | Get-Member -MemberType Property',
        keywords: ['get-member', 'what fields exist', 'column names', 'inspect an object']
      },
      {
        label: 'Take the bare values of one column',
        command: 'Get-ChildItem | Select-Object -ExpandProperty PROPERTY',
        keywords: ['expandproperty', 'awk print $1', 'strings not objects', 'plain values']
      }
    ],
    placeholders: { PROPERTY: 'A column name, such as Length or FullName' },
    keywords: [
      'awk',
      'cut',
      'awk print $1',
      'select columns',
      'choose fields',
      'only show name',
      'fewer columns',
      'get-member',
      'properties of object'
    ]
  },
  {
    id: 'ps-sort-and-top',
    shell: 'powershell',
    category: 'text',
    task: 'Sort a result and keep the top ten',
    summary:
      'Orders the rows of any PowerShell result by one column, largest first, and keeps the first ten.',
    command: 'Get-ChildItem | Sort-Object PROPERTY -Descending | Select-Object -First 10',
    placeholders: { PROPERTY: 'The column to sort by, such as Length or LastWriteTime' },
    keywords: [
      'sort',
      'sort -r',
      'sort | head',
      'order by',
      'top 10',
      'largest first',
      'highest',
      'ranking',
      'sort descending'
    ]
  },
  {
    id: 'ps-filter-rows',
    shell: 'powershell',
    category: 'text',
    task: 'Keep only the rows that meet a condition',
    summary:
      'Filters a PowerShell result by one of its columns. Swap -like for -eq (equals), -gt (greater than), -lt (less than) or -match (regular expression).',
    command: "Get-ChildItem | Where-Object PROPERTY -like '*TEXT*'",
    variants: [
      {
        label: 'Keep only the rows over 10 MB',
        command: 'Get-ChildItem -File | Where-Object Length -gt 10MB',
        keywords: [
          'where-object length',
          'greater than',
          'files bigger than 10mb',
          'numeric condition'
        ]
      },
      {
        label: 'Filter on two conditions at once',
        command:
          "Get-ChildItem | Where-Object { $_.Name -like '*TEXT*' -and $_.Length -gt NUMBER }",
        keywords: ['-and', '-or', 'compound test', 'name and size together', 'both must be true']
      }
    ],
    placeholders: {
      PROPERTY: 'The column to test, such as Name',
      TEXT: 'What the column should contain',
      NUMBER: 'A size to compare with, such as 1MB'
    },
    keywords: [
      'where',
      'where-object',
      'filter',
      'awk',
      'select where',
      'only matching',
      'condition',
      'greater than',
      'rows where',
      'query results'
    ]
  },
  {
    id: 'ps-output-cut-off',
    shell: 'powershell',
    category: 'text',
    task: 'Stop output being cut off with three dots',
    summary:
      'A table that does not fit is shortened with three dots. Showing each row as a list gives every value its full width.',
    command: 'Get-ChildItem | Format-List',
    variants: [
      {
        label: 'Show every property, not the usual few',
        command: 'Get-Process -Id $PID | Format-List *',
        keywords: ['format-list *', 'all the fields', 'hidden columns', 'full object']
      },
      {
        label: 'Keep the table but wrap long values',
        command: 'Get-Service | Format-Table -AutoSize -Wrap',
        keywords: ['format-table -wrap', 'autosize', 'columns too narrow', 'squeezed text']
      }
    ],
    keywords: [
      'truncated',
      'cut off',
      'three dots',
      'ellipsis',
      '...',
      'cannot see full path',
      'show everything',
      'full output',
      'wider',
      'fl',
      'ft'
    ]
  },
  {
    id: 'ps-page-output',
    shell: 'powershell',
    category: 'text',
    task: 'Read long output a page at a time',
    summary:
      'Pauses after each screenful: Space shows the next page, Enter the next line, Q stops.',
    command: 'Get-Command | Out-Host -Paging',
    variants: [
      {
        label: 'Page any program output with more',
        command: 'ipconfig /all | more',
        keywords: ['| more', 'pipe to a pager', 'works for exe output', 'classic way']
      }
    ],
    keywords: [
      'less',
      'more',
      'pager',
      'scrolls past too fast',
      'too much output',
      'page by page',
      'pause output',
      'read slowly'
    ]
  },
  {
    id: 'ps-read-json',
    shell: 'powershell',
    category: 'text',
    task: 'Read a value out of a JSON file',
    summary:
      'Turns the JSON into an object so you can walk into it with dots, for example .scripts on a package.json.',
    command: '(Get-Content FILE.json -Raw | ConvertFrom-Json).KEY',
    variants: [
      {
        label: 'Show a whole JSON file as an object',
        command: 'Get-Content FILE.json -Raw | ConvertFrom-Json',
        keywords: ['convertfrom-json', 'pretty print', 'jq .', 'parse a config', 'readable json']
      },
      {
        label: 'Turn any result into JSON',
        command: 'Get-ChildItem | Select-Object Name, Length | ConvertTo-Json',
        keywords: ['convertto-json', 'serialise', 'export for another tool', 'machine readable']
      },
      {
        label: 'Read a CSV file as a table',
        command: 'Import-Csv FILE.csv | Format-Table',
        keywords: ['import-csv', 'spreadsheet', 'columns', 'excel export', 'parse csv']
      }
    ],
    placeholders: {
      FILE: 'The file name without its ending',
      KEY: 'The property to read, such as version or scripts.build'
    },
    keywords: [
      'jq',
      'json',
      'parse json',
      'package.json',
      'read config',
      'convertfrom-json',
      'csv',
      'pretty print json',
      'get value from json'
    ]
  },

  // -------------------------------------------------------------- processes
  {
    id: 'ps-list-processes',
    shell: 'powershell',
    category: 'processes',
    task: 'See what is running',
    summary:
      'Lists every running program with its ID, memory and CPU time. ps is a short name for it.',
    command: 'Get-Process',
    variants: [
      {
        label: 'Top ten programs by memory',
        command:
          "Get-Process | Sort-Object WorkingSet64 -Descending | Select-Object -First 10 Name, Id, @{n='MB';e={[math]::Round($_.WorkingSet64/1MB)}}",
        keywords: ['ram hogs', 'what is eating memory', 'working set', 'out of memory', 'top']
      },
      {
        label: 'Top ten programs by CPU time',
        command:
          'Get-Process | Sort-Object CPU -Descending | Select-Object -First 10 Name, Id, CPU',
        keywords: ['cpu hogs', 'fans spinning', 'computer is slow', 'htop', 'high usage']
      },
      {
        label: 'List running programs with tasklist',
        command: 'tasklist',
        keywords: [
          'tasklist',
          'tasklist /svc',
          'command prompt version',
          'task manager from the terminal'
        ]
      }
    ],
    keywords: [
      'ps',
      'ps aux',
      'top',
      'htop',
      'tasklist',
      'task manager',
      'running programs',
      'what is using memory',
      'what is using cpu',
      'computer is slow',
      'gps'
    ]
  },
  {
    id: 'ps-find-process',
    shell: 'powershell',
    category: 'processes',
    task: 'Check whether a program is running',
    summary:
      'Looks for running programs whose name contains the word, without the .exe. If it prints nothing, no such program is running.',
    command: "Get-Process -Name '*NAME*'",
    variants: [
      {
        label: 'Show which program file is running',
        command: "Get-Process -Name '*NAME*' | Select-Object Id, Name, Path",
        keywords: ['exe location', 'which node is this', 'full path of a process', 'where from']
      },
      {
        label: 'See when a process started',
        command: "Get-Process -Name '*NAME*' | Select-Object Id, Name, StartTime",
        keywords: ['starttime', 'how long has it been up', 'since when', 'stale process']
      }
    ],
    placeholders: { NAME: 'Part of the program name, such as node or chrome' },
    keywords: [
      'pgrep',
      'pidof',
      'ps | grep',
      'is it running',
      'find process',
      'process id',
      'pid',
      'is node running',
      'still running',
      'tasklist | findstr'
    ]
  },
  {
    id: 'ps-process-command-line',
    shell: 'powershell',
    category: 'processes',
    task: 'See how a running program was started',
    summary:
      "Shows the full command line and the parent of each matching process, which tells ten node.exe rows apart. Other users' processes show an empty command line unless the shell is elevated.",
    command:
      'Get-CimInstance Win32_Process -Filter "Name=\'NAME.exe\'" | Select-Object ProcessId, ParentProcessId, CommandLine | Format-List',
    variants: [
      {
        label: 'Search every command line for a word',
        command:
          "Get-CimInstance Win32_Process | Where-Object CommandLine -like '*TEXT*' | Select-Object ProcessId, Name, CommandLine | Format-List",
        keywords: ['ps -ef | grep', 'which script is running', 'by argument', 'find the culprit']
      }
    ],
    placeholders: {
      NAME: 'The program name without .exe, such as node',
      TEXT: 'Text to look for, such as a script name'
    },
    keywords: [
      'ps -ef',
      'ps aux',
      'command line of process',
      'which node is which',
      'arguments',
      'parent process',
      'ppid',
      'who started this',
      'process tree',
      'wmic process'
    ]
  },
  {
    id: 'ps-stop-process',
    shell: 'powershell',
    category: 'processes',
    task: 'Force a program to close',
    summary:
      'Ends every running program of that name, without the .exe. Reach for it when a window has frozen or a server will not stop.',
    command: 'Stop-Process -Name NAME',
    variants: [
      {
        label: 'Preview what a stop would close',
        command: 'Stop-Process -WhatIf -Name NAME',
        keywords: ['-whatif', 'dry run', 'which ones would go', 'check before killing']
      },
      {
        label: 'Stop one process by its ID',
        command: 'Stop-Process -Id PROCESS_ID',
        keywords: ['kill pid', 'by pid', 'one instance only', 'single process']
      },
      {
        label: 'Force a stubborn process to stop',
        command: 'Stop-Process -Force -Id PROCESS_ID',
        keywords: ['kill -9', '-force', 'will not close', 'refuses to die', 'not responding']
      },
      {
        label: 'Stop a program and its child processes',
        command: 'taskkill /F /T /PID PROCESS_ID',
        keywords: ['taskkill /t', 'whole tree', 'children too', 'npm leaves node behind']
      }
    ],
    placeholders: {
      NAME: 'The program name without .exe, such as node',
      PROCESS_ID: 'The number in the Id column of Get-Process'
    },
    keywords: [
      'kill',
      'kill -9',
      'killall',
      'pkill',
      'taskkill',
      'end task',
      'force quit',
      'frozen',
      'not responding',
      'close program',
      'hung',
      'spps'
    ],
    danger: 'The program closes at once and anything unsaved in it is lost.'
  },
  {
    id: 'ps-port-in-use',
    shell: 'powershell',
    category: 'processes',
    task: 'Find out what is using a port',
    summary:
      'Names the program listening on a port, which is the answer to "port 3000 is already in use". If it prints nothing, nothing is listening there.',
    command:
      'Get-NetTCPConnection -LocalPort PORT -State Listen -ErrorAction SilentlyContinue | ForEach-Object { Get-Process -Id $_.OwningProcess } | Select-Object -Unique Id, ProcessName, Path',
    variants: [
      {
        label: 'Find a port owner with netstat',
        command: 'netstat -ano | findstr :PORT',
        keywords: ['netstat -ano', 'last column is the pid', 'classic way', 'lsof -i']
      }
    ],
    placeholders: { PORT: 'The port number, such as 3000' },
    keywords: [
      'lsof -i',
      'lsof -i :3000',
      'netstat',
      'ss -tulpn',
      'port already in use',
      'eaddrinuse',
      'address already in use',
      'what is on port 3000',
      'who is using port',
      'port taken',
      'localhost',
      'fuser'
    ]
  },
  {
    id: 'ps-free-port',
    shell: 'powershell',
    category: 'processes',
    task: 'Stop whatever is holding a port',
    summary:
      'Finds the program listening on the port and ends it, so your own server can start. Check what it is first with "Find out what is using a port".',
    command:
      'Get-NetTCPConnection -LocalPort PORT -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -Unique | ForEach-Object { Stop-Process -Id $_ -Force }',
    variants: [
      {
        label: 'Preview stopping the program on a port',
        command:
          'Get-NetTCPConnection -LocalPort PORT -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -Unique | ForEach-Object { Stop-Process -Id $_ -Force -WhatIf }',
        keywords: [
          '-whatif',
          'dry run',
          'check first',
          'what holds it',
          'which pid would be stopped'
        ]
      }
    ],
    placeholders: { PORT: 'The port number, such as 3000' },
    keywords: [
      'kill port',
      'free port',
      'kill-port',
      'fuser -k',
      'npx kill-port',
      'port already in use',
      'eaddrinuse',
      'release port',
      'stop dev server',
      'kill $(lsof -t -i:3000)'
    ],
    danger: 'The program holding the port is ended at once, whatever it is, and loses unsaved work.'
  },
  {
    id: 'ps-cancel-command',
    shell: 'powershell',
    category: 'processes',
    task: 'Stop a command that is running right now',
    summary:
      'This is a key press, not something to paste: hold Ctrl and press C in the terminal. It interrupts the running command and gives you the prompt back. Press it twice if the first is ignored.',
    command: 'Ctrl+C',
    keywords: [
      'cancel',
      'abort',
      'interrupt',
      'stop command',
      'how do i stop this',
      'stuck',
      'it will not stop',
      'exit running program',
      'break',
      'sigint',
      'quit server',
      'get my prompt back'
    ]
  },
  {
    id: 'ps-run-in-background',
    shell: 'powershell',
    category: 'processes',
    task: 'Run something without blocking the prompt',
    summary:
      'Starts the program in a window of its own, so this prompt stays free. Closing that window stops it.',
    command: "Start-Process PROGRAM -ArgumentList 'ARGUMENTS'",
    variants: [
      {
        label: 'Start a hidden background job',
        command: '$job = Start-Job { Test-Connection 1.1.1.1 -Count 20 }',
        keywords: ['start-job', 'nohup', 'no window', 'run while i carry on']
      },
      {
        label: 'Read what a job has printed so far',
        command: 'Receive-Job $job',
        keywords: [
          'receive-job',
          'job output',
          'is it finished yet',
          'results of a background task'
        ]
      },
      {
        label: 'List jobs, then stop and clear them',
        command: 'Get-Job; Get-Job | Remove-Job -Force',
        keywords: ['get-job', 'remove-job', 'tidy up', 'what is still running']
      },
      {
        label: 'Send a command to the background with &',
        command: 'npm run dev &',
        keywords: ['&', 'ampersand', 'bash habit', 'powershell 7 only', 'detach']
      }
    ],
    placeholders: {
      PROGRAM: 'The program to start, such as npm',
      ARGUMENTS: 'What you would type after it, such as run dev'
    },
    keywords: [
      '&',
      'nohup',
      'bg',
      'background',
      'start-job',
      'run in new window',
      'keep using terminal',
      'detach',
      'run server and keep typing',
      'jobs',
      'fg'
    ]
  },

  // ---------------------------------------------------------------- network
  {
    id: 'ps-ping',
    shell: 'powershell',
    category: 'network',
    task: 'Check whether a site or machine answers',
    summary:
      'Sends four pings and shows how long each reply took. No replies means it is down, blocked or unreachable.',
    command: 'Test-Connection -Count 4 HOST',
    variants: [
      {
        label: 'Is my internet working at all?',
        command: 'Test-Connection 1.1.1.1 -Count 2',
        keywords: ['ping 1.1.1.1', 'am i online', 'wifi dropped', 'no connection', 'cloudflare']
      },
      {
        label: 'Ping a host the classic way',
        command: 'ping HOST',
        keywords: ['ping', 'icmp', 'round trip time', 'four packets']
      }
    ],
    placeholders: { HOST: 'A name or address, such as github.com' },
    keywords: [
      'ping',
      'is it up',
      'is my internet working',
      'connection test',
      'latency',
      'no internet',
      'offline',
      'reach server',
      'network down',
      'timed out'
    ]
  },
  {
    id: 'ps-test-port',
    shell: 'powershell',
    category: 'network',
    task: 'Check whether a port is open',
    summary:
      'Tries a real TCP connection and reports TcpTestSucceeded True or False. It answers "is the server down or is it just me" better than ping, which many servers ignore.',
    command: 'Test-NetConnection HOST -Port PORT',
    variants: [
      {
        label: 'Is my own dev server answering?',
        command: 'Test-NetConnection localhost -Port PORT',
        keywords: ['localhost 3000', 'is it up yet', 'connection refused', 'local api']
      }
    ],
    placeholders: {
      HOST: 'A name or address, such as github.com',
      PORT: 'The port, such as 443 or 22'
    },
    keywords: [
      'nc -zv',
      'netcat',
      'telnet',
      'port open',
      'port check',
      'connection refused',
      'firewall',
      'can i reach',
      'tnc',
      'is the server up',
      'econnrefused'
    ]
  },
  {
    id: 'ps-my-ip',
    shell: 'powershell',
    category: 'network',
    task: 'Find my IP address',
    summary:
      'Lists the address of each network adapter. The one starting 192.168 or 10 is your address on the local network; 169.254 means that adapter is not connected.',
    command: 'Get-NetIPAddress -AddressFamily IPv4 | Select-Object InterfaceAlias, IPAddress',
    variants: [
      {
        label: 'Full network report with ipconfig',
        command: 'ipconfig',
        keywords: ['ipconfig', 'ifconfig', 'adapters', 'subnet mask', 'classic report']
      },
      {
        label: 'Show the IP and the router gateway',
        command:
          "Get-NetIPConfiguration | Select-Object InterfaceAlias, @{n='IPv4';e={$_.IPv4Address.IPAddress}}, @{n='Gateway';e={$_.IPv4DefaultGateway.NextHop}}",
        keywords: ['default gateway', 'ip route', '192.168.1.1', 'router page', 'network config']
      },
      {
        label: 'My public address, as the internet sees it',
        command: 'Invoke-RestMethod https://api.ipify.org',
        keywords: ['public ip', 'external address', 'wan', 'ipify', 'what the server sees']
      }
    ],
    keywords: [
      'ipconfig',
      'ifconfig',
      'ip a',
      'ip addr',
      'my ip',
      'local ip',
      'public ip',
      'what is my ip',
      'network address',
      'gateway',
      'router address',
      'hostname -i'
    ]
  },
  {
    id: 'ps-download-file',
    shell: 'powershell',
    category: 'network',
    task: 'Download a file from a URL',
    summary:
      'Saves what is at the URL to a file. In Windows PowerShell 5.1 the words curl and wget are only aliases of this command, so for the real curl type curl.exe.',
    command: 'Invoke-WebRequest URL -OutFile FILE -UseBasicParsing',
    variants: [
      {
        label: 'Download with curl, following redirects',
        command: 'curl.exe -L -o FILE URL',
        keywords: ['curl -l -o', 'curl.exe', 'wget', 'github release link', 'real curl']
      },
      {
        label: 'Speed up a download in PowerShell 5.1',
        command:
          "$ProgressPreference = 'SilentlyContinue'; Invoke-WebRequest URL -OutFile FILE -UseBasicParsing",
        keywords: ['progresspreference', 'very slow download', 'hide the progress bar', 'faster']
      }
    ],
    placeholders: { URL: 'The full address, starting https://', FILE: 'The name to save it as' },
    keywords: [
      'curl',
      'curl -o',
      'curl --output',
      'wget',
      'download',
      'fetch file',
      'save url',
      'iwr',
      'get file from internet',
      'download is slow'
    ],
    danger: 'A file of the same name is replaced without a question.'
  },
  {
    id: 'ps-call-api',
    shell: 'powershell',
    category: 'network',
    task: 'Call a web API and read the answer',
    summary:
      'Fetches a URL and, when the answer is JSON, hands it back as an object you can read with dots. It is what curl plus jq does elsewhere.',
    command: 'Invoke-RestMethod URL',
    variants: [
      {
        label: 'Send JSON with a POST',
        command:
          'Invoke-RestMethod URL -Method Post -ContentType \'application/json\' -Body \'{"KEY":"VALUE"}\'',
        keywords: ['curl -x post', 'request body', 'content-type', 'webhook', 'push data']
      },
      {
        label: 'Call an API with a bearer token',
        command: "Invoke-RestMethod URL -Headers @{ Authorization = 'Bearer TOKEN' }",
        keywords: ['authorization header', 'api key', 'oauth', '401 unauthorized', 'curl -h']
      },
      {
        label: 'Fetch only the status code and headers',
        command: 'curl.exe -sI URL',
        keywords: ['curl -i', 'head request', '200 or 404', 'response headers']
      },
      {
        label: 'Fetch the raw text of a URL',
        command: 'curl.exe -s URL',
        keywords: ['curl -s', 'plain body', 'no parsing', 'see what it returns']
      }
    ],
    placeholders: {
      URL: 'The full address, starting https://',
      KEY: 'A field name in the JSON you send',
      VALUE: 'Its value',
      TOKEN: 'Your API key or token'
    },
    keywords: [
      'curl',
      'curl -x post',
      'curl -h',
      'api',
      'rest',
      'http request',
      'get request',
      'post json',
      'irm',
      'test endpoint',
      'httpie',
      'status code'
    ]
  },
  {
    id: 'ps-dns-lookup',
    shell: 'powershell',
    category: 'network',
    task: 'Look up the address behind a domain name',
    summary:
      'Asks DNS which IP addresses a name points to, which shows whether a new domain has taken effect yet.',
    command: 'Resolve-DnsName HOST',
    variants: [
      {
        label: 'Look up an MX, TXT or CNAME record',
        command: 'Resolve-DnsName -Type TXT HOST',
        keywords: ['dig txt', 'spf record', 'mail server', 'domain verification', 'record type']
      },
      {
        label: 'Ask a particular DNS server',
        command: 'Resolve-DnsName -Server 1.1.1.1 HOST',
        keywords: ['dig @1.1.1.1', 'cloudflare resolver', 'bypass my router', 'propagation check']
      },
      {
        label: 'Look up a domain with nslookup',
        command: 'nslookup HOST',
        keywords: ['nslookup', 'classic tool', 'dig', 'quick lookup']
      }
    ],
    placeholders: { HOST: 'The domain name, such as github.com' },
    keywords: [
      'nslookup',
      'dig',
      'host',
      'dns',
      'domain ip',
      'resolve name',
      'mx record',
      'txt record',
      'dns propagation',
      'could not resolve host',
      'enotfound'
    ]
  },
  {
    id: 'ps-listening-ports',
    shell: 'powershell',
    category: 'network',
    task: 'List every port this PC is listening on',
    summary: 'Shows each open port with the program that owns it, lowest port first.',
    command:
      "Get-NetTCPConnection -State Listen | Sort-Object LocalPort | Select-Object LocalPort, OwningProcess, @{n='Process';e={(Get-Process -Id $_.OwningProcess).ProcessName}} -Unique",
    variants: [
      {
        label: 'List ports and connections with netstat',
        command: 'netstat -ano',
        keywords: ['netstat -ano', 'established connections', 'classic way', 'everything at once']
      }
    ],
    keywords: [
      'netstat',
      'netstat -tulpn',
      'ss -tulpn',
      'lsof -i',
      'open ports',
      'listening ports',
      'which ports are in use',
      'what servers are running',
      'local servers',
      'port list'
    ]
  },
  {
    id: 'ps-flush-dns',
    shell: 'powershell',
    category: 'network',
    task: 'Clear the DNS cache',
    summary:
      'Makes Windows forget the addresses it has remembered, so a site that has just moved is looked up afresh. Harmless: the cache refills by itself.',
    command: 'ipconfig /flushdns',
    variants: [
      {
        label: 'Clear the DNS cache in PowerShell',
        command: 'Clear-DnsClientCache',
        keywords: [
          'clear-dnsclientcache',
          'flush the resolver cache',
          'stale dns entry',
          'still going to the old server'
        ]
      }
    ],
    keywords: [
      'flush dns',
      'dns cache',
      'site not loading',
      'old ip',
      'reset dns',
      'resolvectl flush-caches',
      'dscacheutil',
      'wrong server',
      'domain moved'
    ]
  },
  {
    id: 'ps-trace-route',
    shell: 'powershell',
    category: 'network',
    task: 'See where a connection gets stuck',
    summary:
      'Lists each router between you and the destination with its delay. Where the replies stop or slow down is where the trouble is.',
    command: 'tracert HOST',
    variants: [
      {
        label: 'Trace a route with Test-NetConnection',
        command: 'Test-NetConnection -TraceRoute HOST',
        keywords: ['tnc -traceroute', 'hops', 'cmdlet version', 'network path']
      }
    ],
    placeholders: { HOST: 'A name or address, such as github.com' },
    keywords: [
      'traceroute',
      'tracert',
      'mtr',
      'route',
      'hops',
      'slow connection',
      'where is it failing',
      'network path',
      'lag'
    ]
  },
  {
    id: 'ps-open-url',
    shell: 'powershell',
    category: 'network',
    task: 'Open a web page in my browser',
    summary:
      'Opens the address in your default browser, which is handy for localhost while a dev server runs.',
    command: "Start-Process 'URL'",
    variants: [
      {
        label: 'Open a local dev server in the browser',
        command: "Start-Process 'http://localhost:PORT'",
        keywords: ['localhost 3000', 'vite url', 'open the app', 'preview my site']
      }
    ],
    placeholders: {
      URL: 'The full address, starting https://',
      PORT: 'The port your server printed, such as 3000'
    },
    keywords: [
      'open',
      'xdg-open',
      'start',
      'open browser',
      'open link',
      'launch website',
      'open localhost',
      'default browser',
      'view site'
    ]
  },

  // ----------------------------------------------------------------- system
  {
    id: 'ps-disk-space',
    shell: 'powershell',
    category: 'system',
    task: 'See how much disk space is free',
    summary: 'Lists each drive with the gigabytes used and free.',
    command: 'Get-PSDrive -PSProvider FileSystem',
    variants: [
      {
        label: 'Show each volume, size and health',
        command: 'Get-Volume',
        keywords: ['get-volume', 'partitions', 'drive letters', 'file system', 'total capacity']
      }
    ],
    keywords: [
      'df',
      'df -h',
      'free space',
      'disk space',
      'disk full',
      'storage left',
      'how full is my drive',
      'list drives',
      'drive letters',
      'no space left on device',
      'enospc'
    ]
  },
  {
    id: 'ps-memory',
    shell: 'powershell',
    category: 'system',
    task: 'See how much memory is free',
    summary: 'Prints the total and the currently free RAM in gigabytes.',
    command:
      "Get-CimInstance Win32_OperatingSystem | Select-Object @{n='TotalGB';e={[math]::Round($_.TotalVisibleMemorySize/1MB,1)}}, @{n='FreeGB';e={[math]::Round($_.FreePhysicalMemory/1MB,1)}}",
    keywords: [
      'free',
      'free -h',
      'ram',
      'memory',
      'how much ram',
      'out of memory',
      'memory usage',
      'vmstat',
      'available memory',
      'heap out of memory'
    ]
  },
  {
    id: 'ps-powershell-version',
    shell: 'powershell',
    category: 'system',
    task: 'Check which PowerShell I am using',
    summary:
      'Prints the version. 5.1 is Windows PowerShell, the one built into Windows; 7 and up is the newer PowerShell, whose program is pwsh.',
    command: '$PSVersionTable.PSVersion',
    variants: [
      {
        label: 'Is PowerShell 7 installed at all?',
        command: 'Get-Command pwsh',
        keywords: ['pwsh', 'get-command', 'do i have it', 'newer version', 'core']
      }
    ],
    keywords: [
      'version',
      'powershell version',
      'pwsh',
      'which shell',
      'bash --version',
      '5.1 or 7',
      'windows powershell',
      'powershell core',
      'what shell is this'
    ]
  },
  {
    id: 'ps-windows-version',
    shell: 'powershell',
    category: 'system',
    task: 'See my Windows version and hardware',
    summary: 'Prints the Windows edition, build number and whether it is 64-bit.',
    command:
      'Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, OSArchitecture',
    variants: [
      {
        label: 'Show the processor and core count',
        command:
          'Get-CimInstance Win32_Processor | Select-Object Name, NumberOfCores, NumberOfLogicalProcessors',
        keywords: ['lscpu', 'what cpu do i have', 'how many cores', 'threads', 'chip model']
      },
      {
        label: 'Show the graphics card and driver',
        command: 'Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion',
        keywords: ['gpu', 'nvidia', 'video card', 'driver version', 'lspci']
      },
      {
        label: 'Full machine report with systeminfo',
        command: 'systeminfo',
        keywords: ['systeminfo', 'everything at once', 'install date', 'all the specs']
      }
    ],
    keywords: [
      'uname',
      'uname -a',
      'lsb_release',
      'winver',
      'os version',
      'windows build',
      'system info',
      'lscpu',
      'what cpu',
      'what gpu',
      'specs',
      '64 bit'
    ]
  },
  {
    id: 'ps-env-list',
    shell: 'powershell',
    category: 'system',
    task: 'See my environment variables',
    summary: 'Lists every environment variable this shell has, with its value.',
    command: 'Get-ChildItem Env:',
    variants: [
      {
        label: 'Show one variable by its name',
        command: '$env:NAME',
        keywords: ['echo $path', 'printenv name', 'one value', 'is my api key set']
      },
      {
        label: 'Find variables whose name contains a word',
        command: "Get-ChildItem Env: | Where-Object Name -like '*TEXT*'",
        keywords: ['env | grep', 'search for an api key', 'filter the list', 'matching names']
      }
    ],
    placeholders: {
      NAME: 'The variable name, such as USERPROFILE',
      TEXT: 'Part of a name, such as API'
    },
    keywords: [
      'env',
      'printenv',
      'set',
      'echo $var',
      'echo %var%',
      'environment variables',
      'show variable',
      'is my api key set',
      'list env',
      'gci env:'
    ]
  },
  {
    id: 'ps-env-set-session',
    shell: 'powershell',
    category: 'system',
    task: 'Set an environment variable for this session',
    summary:
      'Sets the variable for this terminal and every program started from it. It is forgotten when the terminal closes, which suits an API key you would rather not store.',
    command: "$env:NAME = 'VALUE'",
    variants: [
      {
        label: 'Remove an environment variable',
        command: 'Remove-Item Env:NAME',
        keywords: ['unset', 'clear the value', 'forget my api key', 'delete env var']
      }
    ],
    placeholders: {
      NAME: 'The variable name, such as ANTHROPIC_API_KEY',
      VALUE: 'The value to give it'
    },
    keywords: [
      'export',
      'export var=value',
      'set',
      'set var=value',
      'environment variable',
      'api key',
      'temporary variable',
      'unset',
      'env var',
      'node_env'
    ]
  },
  {
    id: 'ps-env-set-permanent',
    shell: 'powershell',
    category: 'system',
    task: 'Set an environment variable permanently',
    summary:
      'Stores the variable for your Windows user, so every NEW terminal and program sees it. Terminals that are already open, this one included, do not: open a fresh one.',
    command: "[Environment]::SetEnvironmentVariable('NAME', 'VALUE', 'User')",
    variants: [
      {
        label: 'Read back what is stored for my user',
        command: "[Environment]::GetEnvironmentVariable('NAME', 'User')",
        keywords: ['did setx work', 'persisted value', 'user scope', 'check after a restart']
      },
      {
        label: 'Delete a permanent environment variable',
        command: "[Environment]::SetEnvironmentVariable('NAME', $null, 'User')",
        keywords: ['unset for good', 'setx removal', 'clear it permanently', 'take it out']
      }
    ],
    placeholders: { NAME: 'The variable name', VALUE: 'The value to store' },
    keywords: [
      'setx',
      'export in .bashrc',
      'permanent environment variable',
      'persist variable',
      'save api key',
      'user variable',
      'system properties',
      'variable not found after restart',
      'environment variables dialog'
    ],
    danger: 'Any value already stored under that name for your user is replaced.'
  },
  {
    id: 'ps-show-path',
    shell: 'powershell',
    category: 'system',
    task: 'See the folders on my PATH',
    summary:
      'PATH is the list of folders Windows searches when you type a program name. This prints it one folder per line.',
    command: "$env:Path -split ';'",
    keywords: [
      'echo $path',
      'echo %path%',
      'path',
      'show path',
      'path variable',
      'where does it look for commands',
      'command not found',
      'is not recognized'
    ]
  },
  {
    id: 'ps-path-add-session',
    shell: 'powershell',
    category: 'system',
    task: 'Add a folder to PATH for this session',
    summary:
      'Lets you run the programs in that folder by name, until this terminal closes. A safe way to try a PATH change before making it permanent.',
    command: "$env:Path += ';FOLDER'",
    placeholders: { FOLDER: 'The full path of the folder holding the program' },
    keywords: [
      'export path=$path:',
      'add to path',
      'path',
      'temporary path',
      'command not found',
      'is not recognized as the name of a cmdlet',
      'set path',
      'append path'
    ]
  },
  {
    id: 'ps-path-add-permanent',
    shell: 'powershell',
    category: 'system',
    task: 'Add a folder to PATH permanently',
    summary:
      'Appends the folder to the PATH stored for your Windows user. New terminals pick it up; ones that are already open do not.',
    command:
      "[Environment]::SetEnvironmentVariable('Path', [Environment]::GetEnvironmentVariable('Path', 'User') + ';FOLDER', 'User')",
    variants: [
      {
        label: 'Read the PATH stored for your user',
        command: "[Environment]::GetEnvironmentVariable('Path', 'User') -split ';'",
        keywords: [
          'changes nothing',
          'before editing',
          'user scope',
          'what is saved',
          'back it up first'
        ]
      }
    ],
    placeholders: { FOLDER: 'The full path of the folder holding the program' },
    keywords: [
      'add to path',
      'setx path',
      'export path in .bashrc',
      'permanent path',
      'edit path',
      'install location',
      'command not found after install',
      'is not recognized',
      'environment variables dialog'
    ],
    danger:
      'This rewrites your stored user PATH. A slip here stops programs being found, so read it with the variant first.'
  },
  {
    id: 'ps-which',
    shell: 'powershell',
    category: 'system',
    task: 'Find out where a command lives',
    summary:
      'Shows whether a name is a program, an alias or a function, and the file it runs. Note that where on its own means Where-Object in PowerShell: the Windows finder is where.exe.',
    command: 'Get-Command NAME',
    variants: [
      {
        label: 'Show only the path of a program',
        command: '(Get-Command NAME).Source',
        keywords: ['which', 'command -v', 'exe location', 'bare path', 'source']
      },
      {
        label: 'List every copy on PATH, first one wins',
        command: 'Get-Command -All NAME',
        keywords: ['which -a', 'duplicate installs', 'wrong version runs', 'shadowed', 'conflict']
      },
      {
        label: 'Find a program with where.exe',
        command: 'where.exe NAME',
        keywords: ['where.exe', 'whereis', 'classic finder', 'locate an exe']
      }
    ],
    placeholders: { NAME: 'The command, such as node, git or python' },
    keywords: [
      'which',
      'where',
      'whereis',
      'type',
      'command -v',
      'is it installed',
      'which python',
      'which node',
      'path of program',
      'wrong version runs',
      'gcm'
    ]
  },
  {
    id: 'ps-run-as-admin',
    shell: 'powershell',
    category: 'system',
    task: 'Open an administrator PowerShell',
    summary:
      'Opens a new elevated window after the Windows permission prompt. Use it when a command answers "Access is denied" or asks for elevation; it starts in System32, so cd back to your folder.',
    command: 'Start-Process powershell -Verb RunAs',
    variants: [
      {
        label: 'Open an administrator PowerShell 7',
        command: 'Start-Process pwsh -Verb RunAs',
        keywords: ['pwsh as admin', 'elevated pwsh', 'runas', 'newer shell']
      },
      {
        label: 'Open admin PowerShell 7 in this folder',
        command:
          "Start-Process pwsh -Verb RunAs -ArgumentList '-NoExit', '-Command', \"Set-Location '$PWD'\"",
        keywords: ['elevated here', 'keep the current directory', 'admin terminal in my project']
      },
      {
        label: 'Am I running as administrator?',
        command:
          '([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)',
        keywords: ['check elevation', 'true or false', 'uac state', 'is this shell elevated']
      },
      {
        label: 'Run one command as admin with sudo',
        command: 'sudo COMMAND',
        keywords: ['windows sudo', '24h2', 'for developers setting', 'single elevated command']
      }
    ],
    placeholders: { COMMAND: 'The command that needs administrator rights' },
    keywords: [
      'sudo',
      'su',
      'run as administrator',
      'admin',
      'elevated',
      'access denied',
      'access is denied',
      'permission denied',
      'requires elevation',
      'uac',
      'eperm',
      'not allowed'
    ]
  },
  {
    id: 'ps-whoami',
    shell: 'powershell',
    category: 'system',
    task: 'See my user name and computer name',
    summary: 'Prints the account this shell runs as, in the form COMPUTER\\user.',
    command: 'whoami',
    variants: [
      {
        label: 'Show the computer name alone',
        command: '$env:COMPUTERNAME',
        keywords: ['hostname', 'pc name', 'machine name', 'which box am i on']
      },
      {
        label: 'Show the path of my user folder',
        command: '$env:USERPROFILE',
        keywords: ['echo $home', 'c:\\users', 'home directory', 'where my documents live']
      }
    ],
    keywords: [
      'whoami',
      'id',
      'hostname',
      'username',
      'current user',
      'computer name',
      'pc name',
      'home directory',
      'echo $user',
      'echo $home'
    ]
  },
  {
    id: 'ps-services',
    shell: 'powershell',
    category: 'system',
    task: 'See which Windows services are running',
    summary: 'Lists the background services that are running now.',
    command: "Get-Service | Where-Object Status -eq 'Running'",
    variants: [
      {
        label: 'Find a service by part of its name',
        command: "Get-Service -DisplayName '*NAME*'",
        keywords: ['sc query', 'is docker running', 'systemctl status', 'search services']
      },
      {
        label: 'Restart a service (needs admin)',
        command: 'Restart-Service SERVICE_NAME',
        keywords: ['systemctl restart', 'bounce it', 'service is stuck', 'elevated shell']
      }
    ],
    placeholders: {
      NAME: 'Part of the name, such as docker or audio',
      SERVICE_NAME: 'The value in the Name column'
    },
    keywords: [
      'systemctl',
      'systemctl status',
      'service',
      'services.msc',
      'daemon',
      'background service',
      'is docker running',
      'restart service',
      'sc query',
      'gsv'
    ]
  },
  {
    id: 'ps-uptime',
    shell: 'powershell',
    category: 'system',
    task: 'See how long the PC has been on',
    summary: 'Prints the time since Windows last started, as days, hours and minutes.',
    command:
      '(Get-Date) - (Get-CimInstance Win32_OperatingSystem).LastBootUpTime | Select-Object Days, Hours, Minutes',
    variants: [
      {
        label: 'Get the uptime in PowerShell 7',
        command: 'Get-Uptime',
        keywords: ['get-uptime', 'short form', 'pwsh only', 'since boot']
      }
    ],
    keywords: [
      'uptime',
      'last boot',
      'last restart',
      'when did it reboot',
      'how long running',
      'boot time',
      'did it restart',
      'since when'
    ]
  },
  {
    id: 'ps-date',
    shell: 'powershell',
    category: 'system',
    task: 'Get the date and time',
    summary: 'Prints the current date and time, or formats it for a file name or a log line.',
    command: 'Get-Date',
    variants: [
      {
        label: 'A sortable date for a log line',
        command: "Get-Date -Format 'yyyy-MM-dd HH:mm:ss'",
        keywords: ['yyyy-mm-dd', 'timestamp', 'iso style', 'date +%y-%m-%d', 'prefix a line']
      },
      {
        label: 'A date for a file name',
        command: "Get-Date -Format 'yyyyMMdd-HHmmss'",
        keywords: ['no colons', 'backup-20260920', 'dated copy', 'suffix', 'timestamped']
      },
      {
        label: 'Get the time as UTC, ISO 8601',
        command: "(Get-Date).ToUniversalTime().ToString('o')",
        keywords: ['utc', 'zulu', 'universal time', 'api timestamp', 'round trip format']
      }
    ],
    keywords: [
      'date',
      'time',
      'now',
      'timestamp',
      'date +%y-%m-%d',
      'today',
      'current time',
      'utc',
      'iso date',
      'date in file name'
    ]
  },

  // ------------------------------------------------------------------ shell
  {
    id: 'ps-clear-screen',
    shell: 'powershell',
    category: 'shell',
    task: 'Clear the screen',
    summary:
      'Wipes the terminal so the prompt is back at the top. clear and cls are short names for it, and Ctrl+L does the same without typing. Your history is kept.',
    command: 'Clear-Host',
    variants: [
      {
        label: 'Clear the screen with cls',
        command: 'cls',
        keywords: ['cls', 'short way', 'cmd habit', 'wipe it']
      }
    ],
    keywords: [
      'clear',
      'cls',
      'ctrl+l',
      'wipe screen',
      'clean terminal',
      'reset screen',
      'too cluttered',
      'empty screen'
    ]
  },
  {
    id: 'ps-history',
    shell: 'powershell',
    category: 'shell',
    task: 'See the commands I typed before',
    summary:
      'Lists what you ran in this session. The variants read the file that keeps your history across sessions. At the prompt, Up walks back through it and Ctrl+R searches it as you type.',
    command: 'Get-History',
    variants: [
      {
        label: 'Last 50 commands from every session',
        command: 'Get-Content (Get-PSReadLineOption).HistorySavePath -Tail 50',
        keywords: ['history file', 'psreadline', 'what i ran yesterday', 'across windows']
      },
      {
        label: 'Search everything I have ever typed',
        command: "Get-Content (Get-PSReadLineOption).HistorySavePath | Select-String 'TEXT'",
        keywords: ['history | grep', 'ctrl+r', 'that long command', 'find an old one']
      },
      {
        label: 'Run entry number 12 of this session again',
        command: 'Invoke-History 12',
        keywords: ['invoke-history', '!12', 'repeat a numbered line', 'rerun']
      }
    ],
    placeholders: { TEXT: 'Part of the command you are trying to remember' },
    keywords: [
      'history',
      'history | grep',
      'ctrl+r',
      'previous commands',
      'what did i run',
      'command i typed yesterday',
      'recall',
      'doskey /history',
      '!!',
      'repeat command',
      'h'
    ]
  },
  {
    id: 'ps-get-help',
    shell: 'powershell',
    category: 'shell',
    task: 'Get help and examples for a command',
    summary:
      'Prints worked examples for a PowerShell command. Until the help files have been downloaded the local text is thin, so the -Online variant, which opens the documentation page, is often the better read.',
    command: 'Get-Help -Examples NAME',
    variants: [
      {
        label: 'Open the documentation in a browser',
        command: 'Get-Help -Online NAME',
        keywords: ['-online', 'microsoft learn', 'web docs', 'proper manual']
      },
      {
        label: 'Read the whole help in the terminal',
        command: 'Get-Help -Full NAME',
        keywords: ['man', '-full', 'every parameter', 'complete reference']
      },
      {
        label: 'Ask an ordinary program for its help',
        command: 'git --help',
        keywords: ['--help', '-h', '/?', 'usage', 'git help', 'npm help']
      }
    ],
    placeholders: { NAME: 'A PowerShell command, such as Get-ChildItem' },
    keywords: [
      'man',
      'help',
      '--help',
      '/?',
      'tldr',
      'how do i use',
      'manual',
      'documentation',
      'examples',
      'what does this command do',
      'usage'
    ]
  },
  {
    id: 'ps-find-command',
    shell: 'powershell',
    category: 'shell',
    task: 'Find a command by what it does',
    summary:
      'Lists every command whose name contains the word. PowerShell names are Verb-Noun, so searching for the thing (clipboard, archive, service) usually finds it.',
    command: "Get-Command '*WORD*'",
    variants: [
      {
        label: 'List the commands for one kind of thing',
        command: "Get-Command -Noun '*WORD*'",
        keywords: ['-noun', 'by topic', 'related cmdlets', 'everything about services']
      },
      {
        label: 'Search the help text, not only names',
        command: "Get-Help '*WORD*'",
        keywords: ['apropos', 'man -k', 'full text', 'what can do this']
      }
    ],
    placeholders: { WORD: 'A word such as clipboard, zip, service or dns' },
    keywords: [
      'apropos',
      'man -k',
      'compgen -c',
      'search commands',
      'list commands',
      'what is the command for',
      'is there a command',
      'discover',
      'cmdlet',
      'gcm'
    ]
  },
  {
    id: 'ps-make-alias',
    shell: 'powershell',
    category: 'shell',
    task: 'Make a short name for a command',
    summary:
      'An alias is another name for ONE command, with no arguments. For a command with arguments use the function variant. Both last until the terminal closes unless you put them in your profile.',
    command: 'Set-Alias SHORT_NAME COMMAND',
    variants: [
      {
        label: 'Make a shortcut that carries arguments',
        command: 'function SHORT_NAME { COMMAND ARGUMENTS @args }',
        keywords: ['function', "alias ll='ls -la'", 'wrapper', 'alias with flags']
      },
      {
        label: 'Make gs a shortcut for git status',
        command: 'function gs { git status @args }',
        keywords: ['gs', 'alias for git status', 'git shorthand', 'save keystrokes']
      },
      {
        label: 'What does a short name really run?',
        command: 'Get-Alias SHORT_NAME',
        keywords: ['get-alias', 'what is ls here', 'resolve', 'type command', 'behind the name']
      },
      {
        label: 'Which short names does a command have?',
        command: 'Get-Alias -Definition Get-ChildItem',
        keywords: ['get-alias -definition', 'abbreviations', 'gci and dir', 'other spellings']
      }
    ],
    placeholders: {
      SHORT_NAME: 'The name you want to type',
      COMMAND: 'The command it stands for',
      ARGUMENTS: 'What always follows the command'
    },
    keywords: [
      'alias',
      "alias ll='ls -la'",
      'shortcut',
      'short name',
      'abbreviation',
      'doskey',
      'custom command',
      'unalias',
      'what is ls',
      'function'
    ]
  },
  {
    id: 'ps-edit-profile',
    shell: 'powershell',
    category: 'shell',
    task: 'Run something every time PowerShell starts',
    summary:
      'Opens your profile script in Notepad, making it first if there is none. Whatever you put in it (aliases, functions, variables) runs at the start of every new session. Windows PowerShell 5.1 and PowerShell 7 each have their own.',
    command:
      'if (!(Test-Path $PROFILE)) { New-Item $PROFILE -ItemType File -Force }; notepad $PROFILE',
    variants: [
      {
        label: 'Where is my profile script?',
        command: '$PROFILE',
        keywords: ['$profile', 'bashrc equivalent', 'startup file path', 'which file is it']
      },
      {
        label: 'Load the profile again after editing',
        command: '. $PROFILE',
        keywords: ['source ~/.bashrc', 'dot sourcing', 'apply without restarting', 'reload']
      }
    ],
    keywords: [
      '.bashrc',
      '.zshrc',
      '.bash_profile',
      'profile',
      'startup script',
      'autoexec',
      'keep alias after restart',
      'permanent alias',
      'source ~/.bashrc',
      'on startup',
      'dotfiles'
    ]
  },
  {
    id: 'ps-scripts-disabled',
    shell: 'powershell',
    category: 'shell',
    task: 'Fix "running scripts is disabled on this system"',
    summary:
      'Windows blocks every .ps1 script by default, which is also what stops npm, pnpm and other tools installed as scripts. This lets your own user run local scripts while still refusing unsigned ones from the internet. No administrator rights needed.',
    command: 'Set-ExecutionPolicy RemoteSigned -Scope CurrentUser',
    variants: [
      {
        label: 'Show the execution policy at every level',
        command: 'Get-ExecutionPolicy -List',
        keywords: ['get-executionpolicy', 'what is set now', 'scopes', 'changes nothing']
      },
      {
        label: 'Trust one script you downloaded',
        command: 'Unblock-File SCRIPT',
        keywords: ['unblock-file', 'mark of the web', 'downloaded and blocked', 'zone identifier']
      },
      {
        label: 'Run one script without changing anything',
        command: 'powershell -ExecutionPolicy Bypass -File SCRIPT',
        keywords: ['-executionpolicy bypass', 'one off', 'run a ps1 once', 'powershell -file']
      },
      {
        label: 'Run npm when scripts are blocked',
        command: 'npm.cmd install',
        keywords: ['npm.cmd', 'npm.ps1 cannot be loaded', 'pnpm blocked', 'workaround', 'yarn']
      }
    ],
    placeholders: { SCRIPT: 'The script file, such as install.ps1' },
    keywords: [
      'running scripts is disabled',
      'execution policy',
      'cannot be loaded',
      'is not digitally signed',
      'pssecurityexception',
      'unauthorizedaccess',
      'npm.ps1 cannot be loaded',
      'activate.ps1',
      'npm does not work in powershell',
      'chmod +x',
      'allow scripts',
      'remotesigned',
      'unblock'
    ]
  },
  {
    id: 'ps-run-local-program',
    shell: 'powershell',
    category: 'shell',
    task: 'Run a program in the current folder',
    summary:
      'PowerShell never runs things from the current folder by bare name, which is why a file you can see is "not recognized". Put .\\ in front to say "the one right here".',
    command: '.\\PROGRAM',
    variants: [
      {
        label: 'Run a PowerShell script here',
        command: '.\\SCRIPT.ps1',
        keywords: ['./script.sh', 'run a ps1', 'execute', 'local file', 'dot backslash']
      },
      {
        label: 'Run a program whose path has spaces',
        command: "& 'C:\\Program Files\\FOLDER\\PROGRAM.exe'",
        keywords: ['call operator', '&', 'program files', 'quoted path', 'unexpected token']
      }
    ],
    placeholders: {
      PROGRAM: 'The program file, such as setup.exe',
      SCRIPT: 'The script file name without its .ps1 ending',
      FOLDER: 'The folder the program is in'
    },
    keywords: [
      './',
      './script.sh',
      'is not recognized as the name of a cmdlet',
      'not recognized',
      'command not found',
      'run exe',
      'run script',
      'execute file',
      'call operator',
      'path with spaces',
      'bash script.sh'
    ]
  },
  {
    id: 'ps-chain-commands',
    shell: 'powershell',
    category: 'shell',
    task: 'Run one command after another',
    summary:
      'A semicolon runs the second command whatever happened to the first. The && that tutorials use only exists in PowerShell 7: in 5.1 it is an error, so use the if ($?) variant there.',
    command: 'Get-Location; Get-ChildItem',
    variants: [
      {
        label: 'Run the second only if the first worked',
        command: 'git fetch; if ($?) { git status }',
        keywords: ['&& in 5.1', 'if ($?)', 'on success', 'works in windows powershell']
      },
      {
        label: 'Chain with && (PowerShell 7 only)',
        command: 'npm run build && npm start',
        keywords: ['&&', 'and then', 'bash style', 'npm install && npm run dev', 'on success']
      },
      {
        label: 'Chain with || (PowerShell 7 only)',
        command: "npm run build || Write-Host 'The build failed'",
        keywords: ['||', 'on failure', 'fallback', 'or else', 'if it fails do this']
      }
    ],
    keywords: [
      '&&',
      '||',
      ';',
      'and then',
      'chain commands',
      'two commands on one line',
      "the token '&&' is not a valid statement separator",
      'run in sequence',
      'npm install && npm run dev',
      'one after another'
    ]
  },
  {
    id: 'ps-save-output',
    shell: 'powershell',
    category: 'shell',
    task: "Save a command's output to a file",
    summary:
      'Writes what the command prints into a file instead of the screen. Windows PowerShell 5.1 writes UTF-16 here, which some tools cannot read, so the command names UTF-8.',
    command: 'Get-ChildItem | Out-File -Encoding utf8 FILE',
    variants: [
      {
        label: 'Append output to the end of a file',
        command: 'Get-ChildItem | Out-File -Encoding utf8 -Append FILE',
        keywords: ['>>', 'keep what is there', 'add to a log', 'without overwriting']
      },
      {
        label: 'Show output on screen and save it',
        command: 'npm run build | Tee-Object FILE',
        keywords: ['tee', 'both at once', 'watch and record', 'save a build log']
      },
      {
        label: 'Save errors and warnings as well',
        command: 'npm run build *> FILE',
        keywords: ['2>&1', 'stderr', 'every stream', 'red text missing from the file']
      },
      {
        label: 'Throw the output away entirely',
        command: 'git fetch *> $null',
        keywords: ['/dev/null', '$null', 'nul', 'silence it', 'no output', 'quiet']
      }
    ],
    placeholders: { FILE: 'The file to write, such as output.txt' },
    keywords: [
      '>',
      '>>',
      '2>&1',
      'redirect',
      'tee',
      '/dev/null',
      'save output',
      'write output to file',
      'log to file',
      'capture output',
      'nul',
      'silence output'
    ],
    danger: 'Without -Append the file is replaced if it already exists.'
  },
  {
    id: 'ps-clipboard',
    shell: 'powershell',
    category: 'shell',
    task: "Copy a command's output to the clipboard",
    summary:
      'Puts what the command prints on the clipboard, ready to paste into a chat, an issue or an agent.',
    command: 'git status | Set-Clipboard',
    variants: [
      {
        label: 'Copy a whole file to the clipboard',
        command: 'Get-Content FILE -Raw | Set-Clipboard',
        keywords: ['clip < file', 'paste a config', 'send the contents', 'share a log']
      },
      {
        label: 'Print what is on the clipboard now',
        command: 'Get-Clipboard',
        keywords: ['pbpaste', 'read the clipboard', 'what did i copy', 'paste into the shell']
      },
      {
        label: 'Save the clipboard into a file',
        command: 'Get-Clipboard | Set-Content FILE',
        keywords: [
          'pbpaste >',
          'dump the clipboard',
          'write what i copied',
          'paste into a text file'
        ]
      }
    ],
    placeholders: { FILE: 'The file to copy or write' },
    keywords: [
      'pbcopy',
      'pbpaste',
      'xclip',
      'clip',
      'clip.exe',
      'wl-copy',
      'copy output',
      'copy to clipboard',
      'paste',
      'copy error message',
      'scb',
      'gcb'
    ],
    danger: 'Saving the clipboard into a file replaces what that file held, if it already exists.'
  },
  {
    id: 'ps-time-command',
    shell: 'powershell',
    category: 'shell',
    task: 'Time how long a command takes',
    summary:
      'Runs the command and prints how long it took. The output of the command itself is hidden unless you use the variant.',
    command: 'Measure-Command { npm run build }',
    variants: [
      {
        label: 'Time a command and show its output',
        command: 'Measure-Command { npm run build | Out-Default }',
        keywords: [
          'out-default',
          'measure-command hides the output',
          'where did my output go',
          'time and print the result'
        ]
      },
      {
        label: 'Show only the seconds taken',
        command: '(Measure-Command { npm run build }).TotalSeconds',
        keywords: ['totalseconds', 'one number', 'benchmark', 'elapsed', 'how slow']
      }
    ],
    keywords: [
      'time',
      'time npm run build',
      'how long does it take',
      'duration',
      'benchmark',
      'stopwatch',
      'speed',
      'slow build',
      'elapsed'
    ]
  },
  {
    id: 'ps-exit-code',
    shell: 'powershell',
    category: 'shell',
    task: 'Check whether the last command worked',
    summary:
      'Prints the exit code of the last PROGRAM that ran (git, npm, node): 0 means success and anything else is a failure. For PowerShell commands use the $? variant.',
    command: '$LASTEXITCODE',
    variants: [
      {
        label: 'Did the last command succeed? True or False',
        command: '$?',
        keywords: ['echo $?', 'boolean result', 'any kind of command', 'success flag']
      }
    ],
    keywords: [
      'echo $?',
      'exit code',
      'errorlevel',
      'echo %errorlevel%',
      'return code',
      'did it fail',
      'did it work',
      'status',
      'exit status',
      'non-zero'
    ]
  },
  {
    id: 'ps-run-for-each-file',
    shell: 'powershell',
    category: 'shell',
    task: 'Run a command on every matching file',
    summary:
      'Hands each matching file, one at a time, to the command. Inside the braces $_.FullName is the full path of the current file and $_.Name its name.',
    command: "Get-ChildItem -Filter '*.EXTENSION' | ForEach-Object { COMMAND $_.FullName }",
    variants: [
      {
        label: 'Run a command on files in subfolders',
        command:
          "Get-ChildItem -Recurse -File -Filter '*.EXTENSION' | ForEach-Object { COMMAND $_.FullName }",
        keywords: ['find -exec', 'xargs', 'the whole tree', 'recursive loop']
      },
      {
        label: 'Repeat a command five times',
        command: '1..5 | ForEach-Object { COMMAND }',
        keywords: ['for loop', '1..5', 'n times', 'iterate', 'seq']
      },
      {
        label: 'Repeat every two seconds until Ctrl+C',
        command: 'while ($true) { COMMAND; Start-Sleep -Seconds 2 }',
        keywords: ['watch', 'while true', 'poll', 'keep checking', 'sleep 2', 'loop forever']
      }
    ],
    placeholders: {
      EXTENSION: 'A file ending, such as png',
      COMMAND: 'The command to run on each file'
    },
    keywords: [
      'for loop',
      'for f in *',
      'xargs',
      'find -exec',
      'foreach',
      'batch',
      'loop over files',
      'each file',
      'watch',
      'repeat command',
      'sleep'
    ]
  },
  {
    id: 'ps-preview-whatif',
    shell: 'powershell',
    category: 'shell',
    task: 'Preview what a command would do',
    summary:
      'Most PowerShell commands that change something accept -WhatIf: they print what they WOULD do and touch nothing. It works on PowerShell commands only, not on programs such as git or npm.',
    command: 'COMMAND -WhatIf',
    variants: [
      {
        label: 'Which log files would a delete remove?',
        command: 'Get-ChildItem -Recurse -File -Filter *.log | Remove-Item -WhatIf',
        keywords: ['delete all the log files', 'dry run a cleanup', 'before rm', 'check the list']
      },
      {
        label: 'Be asked to confirm each item',
        command: 'COMMAND -Confirm',
        keywords: ['-confirm', 'rm -i', 'prompt me', 'yes or no per item', 'one at a time']
      }
    ],
    placeholders: {
      COMMAND: 'A PowerShell command that changes something, such as Remove-Item FOLDER -Recurse'
    },
    keywords: [
      'dry run',
      '--dry-run',
      'whatif',
      'simulate',
      'preview',
      'test before running',
      'is this safe',
      'rm -i',
      'confirm',
      'afraid to run'
    ]
  },
  // INSTALLING THE AGENTS, this shell's way (2026-10-13), from the vendors'
  // own pages: code.claude.com/docs/en/setup and github.com/openai/codex.
  {
    id: 'ps-install-claude',
    shell: 'powershell',
    category: 'agents',
    task: 'Install Claude Code',
    summary:
      'The native installer, the recommended way: it keeps Claude Code up to date by itself. No need to run it as administrator. When it finishes, open a new tab and check claude --version.',
    command: 'irm https://claude.ai/install.ps1 | iex',
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
    keywords: ['install claude', 'get claude code', 'set up claude', 'claude code installer', 'download claude', 'anthropic cli', 'irm', 'windows']
  },
  {
    id: 'ps-install-codex',
    shell: 'powershell',
    category: 'agents',
    task: 'Install Codex',
    summary:
      "OpenAI's standalone installer for the Codex CLI. No need to run it as administrator. When it finishes, open a new tab and check codex --version.",
    command: 'powershell -ExecutionPolicy ByPass -c "irm https://chatgpt.com/codex/install.ps1 | iex"',
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
    keywords: ['install codex', 'get codex', 'set up codex', 'codex installer', 'openai cli', 'download codex', 'irm', 'windows']
  }
]
