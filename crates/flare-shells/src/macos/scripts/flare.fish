# Flare shell integration for fish, run by --init-command after the user's config: reports the
# folder (OSC 7, file:///path, percent-encoded, no host: an empty host is always this machine, even
# after the Mac is renamed) before every prompt. The user's files are never changed.
function __flare_osc7 --on-event fish_prompt
    printf '\e]7;file://%s\e\\' (string replace -a -- %2F / (string escape --style=url -- $PWD))
end

# A restored tab that hosted an agent: a Claude session id Flare has checked, or codex:last /
# agy:last. Run once, here, after the user's config has set PATH; then the prompt comes as usual.
if set -q FLARE_RESUME_ID
    set -l r $FLARE_RESUME_ID
    set -e FLARE_RESUME_ID
    switch $r
        case codex:last
            codex resume --last
        case agy:last
            agy -c
        case '*'
            if string match -qr -- '^[0-9A-Fa-f][0-9A-Fa-f-]*$' $r
                claude --resume $r
            end
    end
end
