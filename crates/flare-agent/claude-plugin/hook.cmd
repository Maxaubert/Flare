@echo off
rem FLARE READS THE AGENT STATE FROM THIS. Claude Code runs it for
rem the events in hooks\hooks.json and writes the terminalSequence it prints into
rem its OWN terminal, so the sequence reaches that tab and nothing else. A static
rem echo and no node: blocking events wait for it (MEASURED 20 to 100 ms).
rem The arguments are fixed words from hooks.json: a state, and the error kind
rem of a failure. Nothing from stdin is read or repeated.
rem hooks.json starts it as cmd.exe /d /c call and this file's path (exec form, no
rem shell): a command STRING runs through PowerShell where Git Bash is
rem missing, which cannot parse a quoted path and an argument (MEASURED).
if "%~2"=="" (
  echo {"terminalSequence":"\u001b]777;flare-agent;state=%~1\u0007"}
) else (
  echo {"terminalSequence":"\u001b]777;flare-agent;state=%~1;kind=%~2\u0007"}
)
