# Dictation: what only your hands can check

The suite cannot press a real key, hear a cue, open your microphone at your scaling or send keys to a window
that is really in front. These checks are for the owner, on the installed app, with dictation on and a model
downloaded. Each names what to do and when it passes. A failure is a fix task: write down the app, the step and
what you saw. This is Q6 of the M8 plan (`docs/superpowers/plans/2026-10-19-flare-m8.md`), with the checks the
ignored tests below need.

Set the dictation key to Right Alt in Settings > Dictation for checks 1 and 2 (the default is Ctrl+Win).

## The checks

1. **How the cues sound (O13).** Open Notepad and click into it. Press and hold the key, speak one word, and let
   go. **Pass when:** the start cue sounds as the recording starts and the stop cue as you let go; each is one
   short tone, neither clipped nor loud, and neither is heard twice.

2. **AltGr on a Norwegian keyboard (rule 88).** In a PowerShell tab, type a few characters that need AltGr on your
   layout (for example @, { or the euro sign). **Pass when:** every character appears in the tab, no recording
   starts (no pill appears), and a lone Right Alt press still starts one as before.

3. **The Start menu stays shut after Ctrl+Win.** Hold Ctrl+Win in front of Flare, speak, and let go, with the key on
   hold, then on toggle. Only the real Win key does this; the suite has none. **Pass when:** no Start menu opens
   after any of these, and the dictation itself still works.

4. **Pause and resume against a real player (rule 97).** Start a video or music in a real player (Spotify, or a
   browser video). Press and hold the dictation key, speak, and let go. **Pass when:** the player pauses when the
   recording starts and stays paused after it; it resumes only if it was playing. A player that was already paused
   stays paused.

5. **The widget over a full-screen game, and on each monitor.** With "Dictate from any app" on, start a full-screen
   game or video, press the key and speak. Then do the same on the second monitor. **Pass when:** the widget
   appears on the monitor under the pointer, centred and readable at that monitor's scaling; the game keeps its
   input (the widget takes no focus, so the game does not stop or pause on the press).

6. **An elevated window in front.** Open PowerShell as administrator, click into it, and press the key. **Pass
   when:** nothing happens: no widget, no recording, no text typed. This matches Prism Terminal, because an
   ordinary app is not given the keys sent to an administrator window.

7. **A real dictation with your microphone at 225%.** Set Windows' scaling to 225%, open Flare with a shell tab, and
   dictate a sentence. **Pass when:** the pill and the tab mark sit where they should at 225%, the level bar follows
   your voice, and the text pastes at the cursor with no Enter. Then open Claude in a tab, dictate from another app,
   and pick that tab with its number. **Pass when:** the words are typed into the Claude prompt and sent once, and
   the widget says "Sent to" that tab.

8. **Pass times (parity line 923).** With a Whisper Base model and the Parakeet model downloaded, dictate a short
   sentence three times on each engine (Parakeet on CPU, Parakeet on GPU, Whisper Base) and note the time from letting
   go to the text arriving. **Pass when:** Parakeet takes about 0.72 to 0.86 s on CPU and 0.76 to 0.78 s on GPU, and
   Whisper Base about 0.9 s (Prism Terminal's numbers, docs/two-apps.md). Record what you saw in the PR description.

## The ignored tests (run them by hand, not in the gate)

Five tests are `#[ignore]` because they send real input, install the real keyboard hook, play a sound, or need the
engine and a model on disk. Run one by name, from the worktree, with the same target folder as the build:

```powershell
cargo test -p flare-dictation --target-dir <TARGET> -j 6 -- --ignored <name>
```

| Test | File | Why it is ignored |
|---|---|---|
| `reports_the_binding_key_down_and_up_and_stops_when_told` | `crates/flare-dictation/src/keyhook.rs` | sends F24 (0x87) with `SendInput` to the desktop in front; no application uses that key, but keep focus off anything you need |
| `the_hook_says_other_for_a_key_pressed_while_a_binding_key_is_held` | `crates/flare-dictation/src/keyhook.rs` | sends keys with `SendInput` to the desktop in front |
| `hooks_again_after_it_was_turned_off_the_lock_was_let_go` | `crates/flare-dictation/src/keyhook.rs` | installs the real keyboard hook and takes the user's lock |
| `play_plays_the_start_cue` | `crates/flare-dictation/src/cues.rs` | plays a sound (check 1) |
| `the_real_host_answers_like_parakeet_cli` | `crates/flare-dictation/tests/host.rs` | needs `vendor/whisper` and the Parakeet q4 model in the fixture cache (`cargo e2e -- --fetch-fixtures`) |

None of these runs in CI or in the e2e gate. A passing run of each is recorded in the PR description.
