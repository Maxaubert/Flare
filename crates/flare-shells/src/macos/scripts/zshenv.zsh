# Flare shell integration for zsh, loaded through ZDOTDIR. It hands ZDOTDIR straight back, so zsh
# reads every later startup file (.zprofile, .zshrc, .zlogin) from the user's own folder, and it
# sources the user's own .zshenv. The user's files are never changed.
if [[ -n "${FLARE_USER_ZDOTDIR+set}" ]]; then
  builtin export ZDOTDIR="$FLARE_USER_ZDOTDIR"
  builtin unset FLARE_USER_ZDOTDIR
else
  builtin unset ZDOTDIR
fi
typeset -g __flare_hooks="${${(%):-%x}:A:h:h}/flare.zsh"
if [[ -r "${ZDOTDIR:-$HOME}/.zshenv" ]]; then
  builtin source "${ZDOTDIR:-$HOME}/.zshenv"
fi
if [[ -o interactive && -r "$__flare_hooks" ]]; then
  builtin source "$__flare_hooks"
fi
builtin unset __flare_hooks
