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

9. **A long text, and Flare in front (rule 1145).** Dictate from another app for about a minute (a long text that
   Claude shows as "[Pasted text #N]"), pick a Claude tab. **Pass when:** the card says "Sent to" that tab only after
   Claude has started on it; if the prompt still holds the words, the card says "Typed into X, press Enter there"
   instead. Then click a Flare Claude tab so Flare is in front, press Ctrl+Win (Ctrl+Cmd on a Mac), speak, press it again.
   **Pass when:** the words are typed at that tab's cursor with no picker. After a 5-minute recording that the cap
   stopped, one press starts a new recording.

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

## macOS (port phase 7)

The suite on a Mac never opens the real microphone, never asks for a permission and never shows a window: the
microphone is a WAV through the file seam, the access is only read. These checks are the rest, on a Mac, with a
model downloaded. Until phase 8 ships a signed `Flare.app` with `NSMicrophoneUsageDescription`, run them from a dev
build in a terminal: macOS then asks for the microphone for the terminal app (Terminal, Ghostty, Flare's own), and
that is the switch the checks below turn.

M1. **The access row.** Turn the terminal's switch off in System Settings > Privacy & Security > Microphone, start
    Flare and open Settings > Dictation. **Pass when:** the microphone row says "Microphone access is off. Turn it on
    in System Settings > Privacy & Security > Microphone." before you press anything, the Test button says the same,
    and no system prompt appears. Turn it back on, reopen Settings: the line is gone (each listing reads the
    access again, so the old "off" line never stays after the grant).

M2. **The first ask.** On a Mac (or a user) where the terminal was never asked about the microphone, press Test.
    **Pass when:** macOS asks once; after Allow the meter moves; after Don't Allow the row says the access is off and a
    second Test does not ask again.

M3. **The real microphone by hand.** Run the ignored test:

    ```sh
    cargo test -p flare-dictation --lib -- --ignored the_real_default_microphone_lists_and_records_a_second --nocapture
    ```

    **Pass when:** it lists every input you have (built-in, a USB microphone, AirPods) by name, records a second,
    prints a level above 0 while you speak, and passes.

M4 and M5 need the engine on the Mac to hear anything: build it first (M10). M6 to M9 check the key from any app
(step 2) and need no engine: the widget's "Listening" and the crumbs in `flare --diag` show the key working.

M4. **Following the default, and a device that goes away.** With a USB microphone plugged in, choose "System default"
    in Settings, dictate a sentence, switch the default input in System Settings > Sound, dictate again. Then choose
    the USB microphone by name, start a recording and unplug it. **Pass when:** each sentence comes from the default
    of that moment; the unplugged recording ends with what was heard and the pill says no microphone was found; a
    press after that with the USB one still chosen records from the default.

M5. **Two copies.** A Mac copy serves the link only once its key hook is Ready, so first set the key to a chord
    (Control+Shift+D, which needs no permission) or grant Input Monitoring (M7): with the default Control+Command and
    no Input Monitoring neither copy takes the key and neither serves. Start two dev builds of Flare (two profiles).
    **Pass when:** dictating from the one that holds the
    key lists the other's tabs and sends into them, and
    `ls -l "$(getconf DARWIN_USER_TEMP_DIR)flare-dictation"` shows one socket, mode `srw-------`, in a folder of mode
    `drwx------`; quitting the serving copy removes its socket and the other takes over.

M6. **A chord needs no permission.** In Settings > Dictation set the key to Control+Shift+D (or F9, with Fn held or
    "Use F1, F2, etc. keys as standard function keys" on), "Dictate from any app" on. Put Safari or Finder in front.
    **Pass when:** holding the chord starts listening and letting go stops, no system prompt appears, the D never
    reaches the app in front, and the row names no permission. Then try to capture Control+Command+Q, Control+Command+F,
    Control+Command+Space and Control+Left in Settings. **Pass when:** each is refused (the screen never locks, no
    emoji picker or Space switch is taken over), and Control+Command+K is taken.

M7. **A modifier key and Input Monitoring.** Set the key back to the default (Control+Command, the row shows it as
    the Mac's symbols) with the terminal NOT in System Settings > Privacy & Security > Input Monitoring. **Pass
    when:** the "Dictate from any app" row says it needs Input Monitoring and names that pane, no system prompt
    appears, and the key still dictates in Flare's own window. Add the terminal to Input Monitoring (macOS may want
    the terminal restarted). **Pass when:** within a second of Flare seeing the grant the key works with another app
    in front, and the row's line is gone at the next Settings build. Then remove the terminal from Input Monitoring
    again while Flare runs. **Pass when:** within about a second the key stops working with another app in front and
    works again in Flare's own window (the tap ended and let the lock go), and the row names the pane again.

M8. **Right Option on a Norwegian layout.** Set the key to Right Option and the input source to Norwegian. In TextEdit
    type Option+2 (@) and Option+Shift+7 (\) with the right Option key. **Pass when:** the characters type and nothing
    starts listening; holding Right Option alone starts listening and its release stops it; the left Option alone does
    nothing.

M9. **The picker from another app, and Esc.** With a Claude tab open, dictate with another app in front. **Pass
    when:** the list shows, pressing 1 sends to the first row, Esc closes it ("Not sent"), and while it is open Cmd+2
    in a browser still switches tabs; after it closes, 1 to 9 type in the app in front again. Esc during a recording
    cancels it. In Settings, "Pause media while dictating" is off and dimmed, saying a Mac lets no app do it.

M10 to M12 check the engines (step 3). The engine is built from whisper.cpp's pinned source (no macOS binary is
released); until phase 8 puts it in the bundle, the app finds it in a `whisper` folder beside its executable. Build
it once (a few minutes; CMake and the Xcode command line tools are needed) and the app with it:

```sh
bash tools/build-whisper-macos.sh target/debug/whisper vendor/macos/clips
cargo build -p flare -p flare-dictation
```

M10. **A real pass on Metal.** Fetch the pinned tiny model (78 MB) and run the ignored test that drives the engine:

    ```sh
    mkdir -p vendor/macos/models
    curl -fL -o vendor/macos/models/ggml-tiny.bin \
      https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-tiny.bin
    FLARE_WHISPER_DIR=target/debug/whisper FLARE_EXPECT_METAL=1 \
      cargo nextest run -p flare-dictation --test metal_real --run-ignored only --no-capture
    ```

    **Pass when:** both tests pass, the first prints "the pass ran on Some(Gpu) (fell back: false)" and hears
    "ask not what your country can do for you"; the second says the Parakeet host loaded the built libraries.
    `xattr target/debug/whisper/* vendor/macos/models/ggml-tiny.bin` prints nothing Flare put there (curl adds no
    quarantine either; a model Flare downloads itself never has one).

M11. **Dictating with a real model.** In Settings > Dictation download Whisper Base (148 MB) and use it, then
    Parakeet v3 (669 MB). **Pass when:** Large v3 Turbo is the one marked Recommended, no row says "Slow without GPU
    acceleration" and there is no GPU acceleration card; each model types what you say into the tab in front; the
    second Parakeet pass answers well under a second (the resident host, `ps -ax | grep flare-parakeet-host` shows
    one); `ls ~/Library/Application\ Support/FlareDictation/models` lists the models and nothing of a clip is
    anywhere on disk.

M12. **Nothing outlives Flare.** While dictating with Whisper (a `whisper-server` runs), force quit Flare (Activity
    Monitor > Force Quit, or `kill -9 <pid>`). **Pass when:** within a second `pgrep -fl 'whisper-server|parakeet'`
    prints nothing, and no `/bin/sh` reaper is left either (allow 3 s: a reaper whose pipe another process still
    holds notices Flare is gone within 2 s by itself).
