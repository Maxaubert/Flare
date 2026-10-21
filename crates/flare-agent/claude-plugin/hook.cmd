@echo off
rem FLARE READS THE AGENT STATE FROM THIS. Claude Code runs it for
rem the events in hooks\hooks.json and writes the terminalSequence it prints into
rem its OWN terminal, so the sequence reaches that tab and nothing else. A static
rem echo and no node: blocking events wait for it (MEASURED 20 to 100 ms).
rem The arguments are fixed words from hooks.json: a state, then the error kind
rem of a failure, or +sid. Nothing from stdin is read or repeated.
rem hooks.json starts it as cmd.exe /d /c call and this file's path (exec form, no
rem shell): a command STRING runs through PowerShell where Git Bash is
rem missing, which cannot parse a quoted path and an argument (MEASURED).
rem
rem +sid: the turn's own hooks also name the Claude session, so the app knows
rem which transcript is this tab's (dictation checks it before it types). The id is
rem Claude's CLAUDE_CODE_SESSION_ID (the transcript's file name). It is read with
rem DELAYED expansion, which is inert to & | ^ ) and quotes, and printed only when
rem it is 36 characters of 0-9 a-f and -; anything else is left out and the state
rem is printed as before, so an odd value never costs the tab its indicator.
setlocal EnableDelayedExpansion
set "FL_ID="
if "%~2"=="+sid" call :sid
if defined FL_ID (
  echo {"terminalSequence":"\u001b]777;flare-agent;state=%~1;session=!FL_ID!\u0007"}
) else if "%~2"=="" (
  echo {"terminalSequence":"\u001b]777;flare-agent;state=%~1\u0007"}
) else if "%~2"=="+sid" (
  echo {"terminalSequence":"\u001b]777;flare-agent;state=%~1\u0007"}
) else (
  echo {"terminalSequence":"\u001b]777;flare-agent;state=%~1;kind=%~2\u0007"}
)
exit /b 0

:sid
set "FL_V=!CLAUDE_CODE_SESSION_ID!"
if not defined FL_V exit /b 0
if "!FL_V:~35,1!"=="" exit /b 0
if not "!FL_V:~36,1!"=="" exit /b 0
rem "x" first, so a value starting with the eol character is still read. The
rem valid id is all delimiters: "x" is then the only token.
set "FL_A="
set "FL_B="
for /f "tokens=1,2 delims=0123456789abcdef-" %%a in ("x!FL_V!") do (
  set "FL_A=%%a"
  set "FL_B=%%b"
)
if not "!FL_A!"=="x" exit /b 0
if defined FL_B exit /b 0
set "FL_ID=!FL_V!"
exit /b 0