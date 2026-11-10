# FLARE READS THE AGENT STATE FROM THIS (macOS). Claude Code runs it for the
# events in hooks/hooks.json and writes the terminalSequence it prints into its
# OWN terminal, so the sequence reaches that tab and nothing else. A static
# printf and no node: blocking events wait for it.
# The arguments are fixed words from hooks.json: a state, then the error kind of
# a failure, or +sid. Nothing from stdin is read or repeated.
# hooks.json starts it as /bin/sh and this file's path (exec form, no shell
# string), so it needs no execute bit and no PATH lookup.
#
# +sid: the turn's own hooks also name the Claude session, so the app knows
# which transcript is this tab's. The id is Claude's CLAUDE_CODE_SESSION_ID (the
# transcript's file name), printed only when it is 36 characters of 0-9 a-f and
# -; anything else is left out and the state is printed as before, so an odd
# value never costs the tab its indicator. The values only ever reach printf as
# arguments, never as its format.
fl_id=
if [ "$2" = "+sid" ]; then
  fl_v=${CLAUDE_CODE_SESSION_ID-}
  case $fl_v in
    *[!0-9a-f-]*) ;;
    ????????????????????????????????????) fl_id=$fl_v ;;
  esac
fi
if [ -n "$fl_id" ]; then
  printf '{"terminalSequence":"\\u001b]777;flare-agent;state=%s;session=%s\\u0007"}\n' "$1" "$fl_id"
elif [ -z "$2" ] || [ "$2" = "+sid" ]; then
  printf '{"terminalSequence":"\\u001b]777;flare-agent;state=%s\\u0007"}\n' "$1"
else
  printf '{"terminalSequence":"\\u001b]777;flare-agent;state=%s;kind=%s\\u0007"}\n' "$1" "$2"
fi
exit 0
