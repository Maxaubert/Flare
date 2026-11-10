# Flare shell integration for fish, run by --init-command after the user's config: reports the
# folder (OSC 7, file:///path, percent-encoded, no host: an empty host is always this machine, even
# after the Mac is renamed) before every prompt. The user's files are never changed.
function __flare_osc7 --on-event fish_prompt
    printf '\e]7;file://%s\e\\' (string replace -a -- %2F / (string escape --style=url -- $PWD))
end
