# Antigravity (agy) as an agent, and agent icons that move: spec and plan

Date: 2026-10-26. Owner: Max. Research: `C:\Users\Admin\Documents\Claude\research\flare\2026-10-26-agy-cli-and-flare-agent-map.md`
(agy 1.4.7 facts, and a file:line map of every place Flare names an agent).

## Owner decisions (2026-10-26)

1. **Support the Antigravity CLI (`agy`)** as a third agent next to Claude Code and Codex. Its tab had no icon.
2. **agy's icon is G1**: the pixel "A" from agy's own banner, in its rainbow colours (`docs/surfaces/agy-icon-mock.html`).
3. **Agent icons move while their tab is working**: Claude **H1** (a slow hop, 1.8 s, with a blink every other landing), Codex
   **O6** (spin 3.5 s while fading 0.55 to 1), agy **A1** (the rainbow flows along the arch, 3 s)
   (`docs/surfaces/agent-motion-mock.html`, `docs/surfaces/agy-motion-mock.html`).
4. **It is a mod's job: plugins may change an agent tab's icon motion from now on.** This amends the binding plugin design
   (section 13: "`tabs.icon.set` works only on tabs with no agent"). The amendment is narrow: a plugin picks a **motion from
   Flare's fixed list**; it never supplies the agent's artwork, never draws a check or dot, and the agent's marks (ring, bar)
   stay Flare's. Written into `docs/surfaces/decisions.md` and `plugins.md`.

## Part 1: agy as an agent (PR 1)

- **Kind**: `AgentKind::Agy` and `DetectedAgent::Agy` (word `"agy"` in `tabs.json`, `tabs.list` and the `agent` event; shown
  as "Antigravity"). Every match listed in the research map gets an arm; the "no file lists the agents by hand" guard stays.
- **Detection**: the program `agy` / `agy.exe` (first token, as for `claude.exe`); add `agy` to the prefilter. One `agy.exe`
  process, no node.
- **State, OSC 7501** (new, generic): parse `ESC ] 7501 ; state=<idle|working|done|blocked|error>[:kind=permission|question|auth]
  [:app=..][:msg=base64] ST` (and `clear`). Map: working -> Working, blocked -> Question, done -> Done, error -> Failed, idle ->
  Idle. Feed it as a third signal source beside hooks and titles, for any agent that sends it (Claude Code may too). Text from
  `msg` is never shown unclean. **Enable it**: every shell Flare starts gets `AGY_CLI_OSC7501=on` (agy asks with `]7501;?`
  first; the reply format is unverified, so the env switch is the reliable path). If a live capture shows the exact query
  reply, answering it is a follow-up.
- **Fallback**: with no 7501 (older agy), the existing output scoring gives working/idle once the poll names the agent.
- **Resume**: save `"agy"` with resume `agy:last`, started as `agy -c` (most recent conversation in that folder). A tab whose
  conversation id is known later may use `agy --conversation <uuid>`; not needed now.
- **Icon**: `IconKind::Agy`, int 3 (settings moves to 4), drawn as 46 pixel cells in 10 columns, each column its own colour
  (a `for` over cells in `AgentIcon`, or one path per colour: the builder picks). Colours go through the colour-literal allow
  file; a Rust copy in `icon.rs` and a pin test like Clawd's. Licence line in `THIRD-PARTY-NOTICES.md` next to Clawd's and
  OpenAI's.
- **Names and keys**: close question, ribbon ("Antigravity is using this page"), decor tooltip, notifications, help entry.
  Keys: agy's paste and newline defaults as Codex's until measured.
- **Not in this PR**: hooks for agy, injecting Flare's MCP server into agy (`agy mcp add` writes the user's global config:
  a separate ask), dictation routing.

## Part 2: icons that move, picked by a mod (PR 2)

- **Flare draws the motions**, frame-driven from the mark clock (`MarkMotion`, pushed in `push_frame`), never Slint
  `animate`. A fixed list: `none`, `hop-blink` (H1), `spin-fade` (O6), `rainbow-flow` (A1, agy only: it needs the colours),
  plus `hop`, `spin`, `breathe` for any agent. Each runs **only while that tab is Working** and is still with Windows'
  animation effects off (the existing `STILL` path). The clock already runs while any tab works, so nothing new ticks idle.
- **Plugin API**: `agents.motion.set {agent: "claude"|"codex"|"agy", motion}` and `agents.motion.clear {agent}`, in a new
  namespace with a new permission `agent-looks` ("Change how agent icons move while they work"), protocol 2 only, asked at
  install like the others. Unknown agent or motion: `bad_params`. Last writer wins; a plugin switched off or removed takes
  its motions with it (the decor rule). Rate limit as decor. `capabilities` lists the motions.
- **Sample mod `agent-moves`** in `docs/surfaces/sample-plugins/` (or wherever the phase 8 samples live): sets Claude
  `hop-blink`, Codex `spin-fade`, agy `rainbow-flow`; a small settings page to pick per agent.
- **No built-in default**: without the mod, icons stay still (today's look).

## Tests

- Part 1: detection (`agy`, `agy.exe`, a path, not "legacy"/"strategy"), the 7501 parser (every state, kind, clear, bad
  base64, split across chunks, inside other OSCs), indicator states from 7501, resume round trip, icon pixels and pin, the
  agent-list guard, tabs.json with `"agy"` read by this Flare, e2e kind name.
- Part 2: each motion's frames at fixed phases (hop height, blink frames, spin angle, flow colour offset), still when not
  working and with effects off, the method's permission and params, clear on plugin off, the sample mod's manifest.
- Live check (caller): a real `agy` run in a Flare tab shows the icon and working/question/done marks; the mod animates.

## Plan

PR 1 on `feat/agy-agent` (built, reviewed, fixed by a workflow), then PR 2 on `feat/agent-motion` from main after PR 1 merges.
Merge questions per PR in chat. No install until Max says so.
