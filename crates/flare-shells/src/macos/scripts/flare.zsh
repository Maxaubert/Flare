# Flare shell integration for zsh: reports the folder (OSC 7, file:///path, percent-encoded)
# before every prompt, so the tab follows cd. No host: zsh reads HOST once at startup, and a Mac
# whose name comes from the network can be renamed mid-session; an empty host is always this machine.
__flare_osc7() {
  emulate -L zsh
  setopt no_multibyte
  local p=$PWD out= c h i
  for (( i = 1; i <= $#p; i++ )); do
    c=$p[i]
    case $c in
      ([a-zA-Z0-9/._~-]) out+=$c ;;
      (*) printf -v h '%%%02X' "'$c"; out+=$h ;;
    esac
  done
  printf '\e]7;file://%s\e\\' "$out"
}

# A restored tab that hosted an agent: the shim kept FLARE_RESUME_ID (a Claude session id Flare has
# checked, or codex:last / agy:last / claude:new for a new blank Claude) in __flare_resume. It runs
# once, at the first prompt, so PATH is the one .zprofile and .zshrc set and `claude` is found as
# when the user types it.
__flare_resume_run() {
  local r=${__flare_resume-}
  __flare_resume=
  case $r in
    (codex:last) codex resume --last ;;
    (agy:last) agy -c ;;
    (claude:new) claude ;;
    (*)
      if [[ -n $r && $r != -* && $r != *[^0-9A-Fa-f-]* ]]; then
        claude --resume "$r"
      fi
      ;;
  esac
}

# Installed at the first prompt, after .zshrc has run, and put last, so a prompt theme that set up
# its own hooks keeps them and Flare's still runs. A restored tab's agent runs BEFORE the first
# folder report: OSC 7 says a prompt with nothing in front of it, and Flare takes a tab's agent
# (and the Claude session its resume named) back at that report, so a report sent just before
# `claude --resume` cost every restored tab its session (rule 1286). The report comes once the
# agent has exited, as after any command.
__flare_install() {
  precmd_functions=(${precmd_functions:#__flare_install})
  precmd_functions=(${precmd_functions:#__flare_osc7} __flare_osc7)
  __flare_resume_run
  __flare_osc7
}
typeset -ag precmd_functions
precmd_functions+=(__flare_install)
