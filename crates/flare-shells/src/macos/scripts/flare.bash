# Flare shell integration for bash, run as the --rcfile. It reads the files a login shell reads
# (/etc/profile, then the first of ~/.bash_profile, ~/.bash_login and ~/.profile), so PATH from
# /etc/paths and path_helper is set, then reports the folder before every prompt (OSC 7,
# file:///path with no host: HOSTNAME is read once at startup and goes stale when the Mac is
# renamed). The user's files are never changed. A restored tab's resume (FLARE_RESUME_ID) is taken
# out of the environment before the login files run and started at the end, with their PATH.
__flare_resume=${FLARE_RESUME_ID-}
unset FLARE_RESUME_ID
if [ -r /etc/profile ]; then . /etc/profile; fi
if [ -r ~/.bash_profile ]; then
  . ~/.bash_profile
elif [ -r ~/.bash_login ]; then
  . ~/.bash_login
elif [ -r ~/.profile ]; then
  . ~/.profile
fi

__flare_osc7() {
  local s=$? LC_ALL=C p=$PWD out= c h i
  for (( i = 0; i < ${#p}; i++ )); do
    c=${p:i:1}
    case $c in
      [a-zA-Z0-9/._~-]) out+=$c ;;
      *) printf -v h '%d' "'$c"; printf -v h '%%%02X' $(( h & 255 )); out+=$h ;;
    esac
  done
  printf '\e]7;file://%s\e\\' "$out"
  return $s
}
PROMPT_COMMAND="__flare_osc7${PROMPT_COMMAND:+;$PROMPT_COMMAND}"

# A restored tab that hosted an agent: a Claude session id Flare has checked, or codex:last / agy:last.
__flare_resume_run() {
  local r=$__flare_resume
  __flare_resume=
  case $r in
    codex:last) codex resume --last ;;
    agy:last) agy -c ;;
    *)
      if [[ -n $r && $r != -* && $r != *[^0-9A-Fa-f-]* ]]; then
        claude --resume "$r"
      fi
      ;;
  esac
}
__flare_resume_run
