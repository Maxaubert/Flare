# Android emulator in the device panel (Windows and macOS): implementation plan (2026-11-22)

Spec: `docs/superpowers/specs/2026-11-22-android-device-design.md`.
Status: draft for Max's approval, together with the spec. The first spikes (A0 to A6) ran on 2026-11-22; Step 0
below holds the ones still open. No PR started.

One gating step (spikes, no PR), then six code PRs. Each builds and passes its own tests. Merge order:
**A1, A2, A3, A4, A5, A6**.
- Step 0 gates: S1 and S3 before A4 (the frame path), S2 before A4 merges (the Mac touch gate, open decision 6), S4
  before A3, S5 before A2 merges.
- A1 (pure model) and A2 (emu host) ship alone; nothing is wired, users see no change.
- A2b (Android setup: Flare downloads the emulator, platform-tools and an image from Google into its own SDK folder
  after the licence click, and writes its own AVD; no Java) ships alone, unwired.
- A3 (agent helper jar) ships alone.
- A4 (Flare side: frames, panel, setup, menu entry) is the first visible change. Mockup first (open decision 7).
- A5 (agent tools for Android, capabilities on Windows) ships after A4.
- A6 (bundle and sign: the emu host and the jar in the Windows installer and in `Flare.app`) can merge right after
  A2, like iOS PR 6, so the release path is proven early.

## Step 0: spikes still open (no PR)

All on AVDs made for the run (`flare-spike` exists on both machines from 2026-11-22; delete it after Step 0), headless,
nothing on Max's screen. Results go into `research/flare/2026-11-22-android-emulator.md`.

| # | Question | Pass when |
|---|---|---|
| S1 | **Done 2026-11-22.** Probe app (barcode each vsync) with the MMAP stream, 15 s: PC 879/879 guest frames shown, 58.7 fps, interval p99 31 ms (Python decoding each frame in the reader), 0 torn; Mac 895/895, 59.7 fps, p99 21 ms, 0 torn. Rows are top-down (not bottom-up): no Y flip. The emulator's screenshot masks the display's rounded corners. `width` scaling with MMAP not yet checked (A2). | Passed (100% shown, 0 torn on both). |
| S2 | **Done 2026-11-22.** Touch to photon with the probe app (flips a band on touch down), 40 touches: PC p50 51 ms, p90 73 ms; Mac p50 45 ms, p90 49 ms. The first spike's 61 ms on the Mac came from the system pointer overlay, not the input path. The gate is measured again in Rust in A4. | Mac passes; PC at the gate (51 vs 50 ms, Python reader). |
| S3 | Zero-copy import. Windows: `OpenExistingHeapFromFileMapping` on the region's file mapping, a placed buffer, `copy_buffer_to_texture` through wgpu-hal DX12 on Flare's adapter. Mac: `newBufferWithBytesNoCopy` on the mapping, wrapped through wgpu-hal Metal. Against `write_texture` on both. | Per OS: the zero-copy route works and its GPU time p99 is under 1 ms at 1080x2400; else `write_texture` CPU time p99 under 1.5 ms. The faster working route is chosen. |
| S4 | Agent helper prototype (`app_process` + `UiAutomation`, JSON over `adb forward`): tree time on Settings and a busy screen, on API 36 (Mac) and 37 (PC), plus `ACTION_SET_TEXT` with æøå and emoji on a View and a Compose field. | Tree p50 under 60 ms warm on both; set-text works on both field kinds or the clipboard fallback does. |
| | **PC done 2026-11-22 (API 37)**: the helper needs `Looper.prepareMainLooper()` first. Settings tree 155 to 220 nodes, round trip p50 2.9 to 4.7 ms warm (first read 63 to 67 ms; `uiautomator dump` took 2.2 s); start 160 to 300 ms. `ACTION_SET_TEXT` put "blåbær ørret 😀" into Settings search in 21 ms (screenshot checked). A helper left running blocks a new one (the new one is killed), so the emu host stops an old helper first. Still to do: the Mac (API 36) and a Compose field. | PC passed. |
| S5 | Token auth and discovery: boot with `-grpc-use-token`, read `grpc.token` from `pid_<pid>.ini` on both OSes, connect with the bearer header; plain `-grpc` refused by Flare's own check; attach to an emulator started by `emulator` alone (Studio's default flags) and see what its discovery file allows. `-idle-grpc-timeout 300` stops it after the client dies. | All four behave as the spec says. |
| | **Partly done 2026-11-22**: with `-grpc-use-token` a call without the header answers UNAUTHENTICATED; the discovery file holds `grpc.token`, `grpc.port`, `grpc.allowlist` on both OSes (Mac: `~/Library/Caches/TemporaryItems/avd/running`). Idle timeout and attaching to a Studio-started emulator still to check. | |
| S6 | Quick Boot: second boot time on both machines with the AVD's own snapshot. So far: the first boot of a new AVD took 139 s on the PC (first-run setup in the guest); later cold boots took 19 s (PC) and 16 s (Mac). | Recorded (no gate). |
| S7 | Flare-style setup without Java: on a clean folder, read `repository2-3.xml` and `sys-img2-3.xml`, download `emulator`, `platform-tools` and the newest stable `google_apis` image for the host CPU, check SHA-1, unpack with `package.xml`, write the licence hash and an AVD `config.ini` by hand, then boot it headless with `ANDROID_SDK_ROOT` and `ANDROID_AVD_HOME` pointing there. On both OSes. | It boots and streams like S1, with no Java and no `sdkmanager` on the path. |
| | **Mac done 2026-11-22** (Python script, standard library only, `env -i` with no Java on the path): emulator 37.3.10, platform-tools 37.0.3, API 37 `google_apis` arm64 image; 2.5 GB in 48 s, every SHA-1 matched; 8.0 GB on disk. The hand-written AVD booted in 30 s. After the first-boot background compile: 60.1 fps, 901/901 frames, p99 21 ms, 0 torn; touch p50 46 ms. New: 37.3.10 rejects a region file outside its discovery folder, `/tmp` or `/tmp/android-<user>` (PERMISSION_DENIED); a `flare` subfolder of the discovery folder works. Windows still to do (with a 37.3.10 emulator, to see the same rule there). | Mac passed. |

## Rule numbers

On origin/main 507db48e the last rule is 1356; open PR #326 takes 1357. This plan takes **1360 to 1371**. Check
`docs/regression-rules.md` and open PRs again before each PR and renumber if another PR took them. New rules go in a
section `## Device panel: Android (2026-11-22)`.

| Rule | Says | PR |
|---|---|---|
| 1360 | Android is a second `DeviceSource` behind the same panel kind, tools and job. Targets are `emu/<avd>`. No tool name, schema key or panel kind names a platform. | A1 |
| 1361 | Flare never ships or mirrors the emulator or system images; it downloads them from Google's own servers into its own SDK folder, only after the person accepts Google's licence in Flare, checks each SHA-1, and never changes the person's own SDK. No Java, no `sdkmanager`, no `avdmanager`. Floors: emulator 37.2.13 on Windows, 37.3.10 on macOS. | A2b |
| 1362 | Flare starts emulators only with `-grpc-use-token` (never plain `-grpc`), `-no-window`, `-gpu host` and `-idle-grpc-timeout 300`; the token is never logged or shown to the agent. | A2 |
| 1363 | Frames come through `streamScreenshot` with MMAP into a region Flare owns (a user-only file in a `flare` folder inside the emulator's discovery folder, the only place 37.3.10 accepts besides `/tmp`; deleted on detach and at start). Frame records travel on the emu host's stdout. | A2, A4 |
| 1364 | Each frame is copied out of the single-buffered region on the GPU at once, off the UI thread, into the device ring; the UI thread only shows the newest finished slot. At most 60 a second; no frame queue. | A4 |
| 1365 | The stream runs only while the panel is seen (open, tab in front, window not minimised or occluded); input, tree, `snapshot` and the agent keep working while it is paused. | A2, A4 |
| 1366 | An emulator Flare booted is shut down at quit (setting, detached, quit never waits) and stops by itself 5 minutes after Flare dies; an emulator Flare did not boot is never shut down. | A2, A4 |
| 1367 | Agent coordinates for Android are dp (pixels / (dpi / 160)); only the emu host converts. `device_look`'s PNG is at dp size. | A1, A5 |
| 1368 | Typing never touches the computer's clipboard: HID keys, then the agent helper's set-text, then the guest clipboard plus paste. | A2, A3 |
| 1369 | The agent helper jar is built from source in the repo, its hash checked in CI, pushed only to emulators (never to a real phone in v1), reached only through `adb forward` to localhost; if it fails the tree falls back to `uiautomator dump` with a "slow tree" mark. | A3 |
| 1370 | No emulator window, taskbar button or Dock icon ever appears; the real e2e scenario checks visible windows before and after. | A2, A4 |
| 1371 | Each source lists its own buttons; `device_press` refuses another source's button with `unavailable`. | A1, A5 |

Existing rules to respect and cite: 34, 40, 41, 229, 1053, 1054, 1105, 1117, and the iOS device rules 1310 to 1323.

## Shared decisions (every PR)

- No ctl protocol bump. `device.list` gains `kind`; `device.launch` and `device.install` take Android values.
- Blocking work on workers (rule 34): every `emulator`, `adb`, `avdmanager` and gRPC call is in the emu host or on a
  worker; the UI thread reads results through `UiMsg::Device`.
- Tests headless only; real emulators only on AVDs made for the run and deleted after.
- One cargo build at a time; shared target; `--bin flare` for the app's tests.
- Windows and macOS CI both run every new test that is not marked for one OS.

## PR A1: the pure model for a second source

Files: `crates/flare-device/src/{target,source,tree,fake,geom,wire}.rs`, `crates/flare/src/device_ctl.rs` (types only).

1. `Target::Emu { avd }` with `emu/` parse, `target_ok`, display name; rename `udid()` to `id()` (callers updated).
2. `Button::{Back, Recents, Power}`; `Buttons` per source (`Source::buttons() -> &'static [Button]`).
3. `Screen.scale` documented as points (iOS) or dp (Android); `geom.rs` tests for dp maps in four rotations.
4. `tree.rs`: a role mapping table for Android classes (`android.widget.Button` and so on, plus `clickable`), so
   `actionable()` works for both.
5. `wire.rs`: `Hello` gets `kind` and a `facts` map (old fields stay for the sim host); new `HostLine::Region` frame
   record; round-trip tests.
6. `SourceEvent::Region { target, seq, t_us, size_px, rotation }`.
7. `FakeSource::android()` preset.
8. `device_ctl.rs`: `Setup` gains the Android states (1 to 4 and 6 of the spec) as data and lines, not yet shown;
   `Starter` takes a `SourceSetup` enum (`Xcode(..)` or `AndroidSdk(..)`) instead of `&Xcode`.
9. Rules 1360, 1367, 1371.

## PR A2: `flare-emu-host`

Files: `crates/flare-device/src/emu/` (new), `src/bin/flare_emu_host.rs`, `src/emu_client.rs` (Flare side, like
`client.rs`), `crates/flare-proc` (Windows guard), `Cargo.toml` (tonic, prost, tokio for this bin only), vendored
`proto/emulator_controller.proto` (Apache-2.0, with its licence header).

1. SDK discovery, version floors, `-list-avds` plus `config.ini`, discovery-file reader, `-accel-check` on Windows.
2. Boot (command line of the spec, on a worker), boot wait, attach, detach, shut down, `getStatus` keep-alive every
   60 s.
3. gRPC client: `streamScreenshot` with MMAP to Flare's region URL; frame records with `FrameCap`; pause and resume
   by cancelling and reopening the stream; fallback to pixels in the message; `getScreenshot` for `snapshot`
   (PNG at dp size, encoded here).
4. Held `streamInputEvent`: touch (two contacts), keys as USB HID, buttons, lift-all on reset.
5. Install and launch through adb (path checks stay on the Flare side, in A5).
6. `--self-test`, `--bench`, `emu-host.log`.
7. Windows `guard_command` in `flare-proc` (kill-on-close job, lifted from `flare-dictation/src/job.rs`; the
   emulator is started outside the job with `CREATE_BREAKAWAY_FROM_JOB`).
8. Flare side: `EmuSource` implements `DeviceSource`, reuses the supervisor and restart counting from `client.rs`.
   Not wired.
9. Tests: parsers with spike fixtures, the command line, the wire protocol, the self-test in CI on both OSes.
10. Rules 1361, 1362, 1363, 1365, 1366, 1368, 1370.

## PR A2b: Android setup (Flare installs what it needs)

Files: `crates/flare-device/src/emu/setup.rs` (new), `emu/avd.rs` (new), `flare_emu_host.rs`.

1. SDK folder per OS; reuse the person's SDK when it is at the floor; never write into it.
2. Package lists from `dl.google.com` (HTTPS), pick `emulator`, `platform-tools` and the newest stable
   `google_apis` image for the host CPU; licence text for the panel.
3. Download on a worker with progress events and cancel; SHA-1 check; unpack to a temporary folder, then rename;
   `package.xml` and `licenses/android-sdk-license` written as the SDK does.
4. Write the "Flare Pixel" AVD (`config.ini` and its `.ini`) under Flare's `avd` folder; `ANDROID_AVD_HOME`
   for Flare's emulators.
5. Windows: `-accel-check`; the elevated `dism` command for [Turn on] (started only from the person's click).
6. Update check (newer stable emulator, floor).
7. Tests: package-list parsing with a saved copy of both XML files, checksum failure, cancel mid-download (nothing
   left behind), AVD file contents; a real download only in S7 and the opt-in real scenario.
8. Rule 1361.

## PR A3: `flare-agent.jar`

Files: `tools/android-agent/` (Java source, build script, probe app), `crates/flare-device/assets/flare-agent.jar`
(+ `.sha256`), `crates/flare-device/src/emu/agent.rs`.

1. The helper: `UiAutomation` connection, tree JSON, set-text, front app, window events, `hello` with version.
2. Build script (`javac` + `d8` from the SDK's build-tools), output checked in; CI job checks the jar matches the
   source hash.
3. Emu host: push, start with `app_process`, `adb forward`, restart once on failure, `uiautomator dump` fallback.
4. The probe app (frame barcode, touch time) for S1, S2 and the real e2e; never bundled.
5. Rule 1369.

## PR A4: the panel shows Android (first visible change)

Mockup first (open decision 7): the header with Back, Home, Recents and the menu; the chooser with both kinds; the
new setup lines. Files: `crates/flare/src/device_frames.rs` (Windows and the region path), `device_ctl.rs`,
`device_glue.rs`, `ui/panel.slint`, `Cargo.toml` (`windows` feature `Win32_Graphics_Direct3D12`).

1. `device_frames.rs`: a region path on both OSes. The frame thread copies each record's region into the ring with
   the import S3 chose (D3D12 heap or Metal no-copy buffer, else `write_texture`), Y flip if S1 says so;
   `on_submitted_work_done` frees slots as for iOS. The stub on Windows becomes real.
2. Flare makes, maps, sizes and deletes the region files.
3. The gate becomes per source: `menu_entry_offered` is true when any source is ready (Xcode on Apple silicon, or
   an Android SDK at the floor with an AVD); the chooser lists both.
4. Setup states shown; Android header buttons; takeover unchanged.
5. Quit shutdown for Flare-booted emulators; `booted.json` holds both kinds.
6. GPU import test per OS (no window); ui_tests for the header and setup states; appctl input tests with the
   Android fake; e2e fake variants.
7. Bench numbers from Max's PC and Mac in the PR (A1 to A3 gates of the spec).
8. Rules 1364, 1365, 1366, 1370 (Flare side).

## PR A5: agent tools for Android

Files: `crates/flare-mcp/src/device.rs`, `crates/flare/src/ctl_device.rs`, `device_agent.rs`, `flare-ctl`.

1. Tool descriptions without platform words; `device_list` `kind`; `device_launch` package names; `device_install`
   `.apk` inside the tab's folder; `device_press` per-source buttons.
2. Capabilities: `device.*` listed on Windows when the Android source is ready.
3. The job and indicators unchanged; dp coordinates through `device_look`.
4. e2e: `deviceAgent` and `mcpDevice` Android variants on both e2e jobs; `deviceRealAndroid` opt-in.
5. `docs/features.md` tool counts per OS.
6. Rules 1367, 1371 (tool side).

## PR A6: bundle and sign

1. Windows: `flare-emu-host.exe` and `flare-agent.jar` in the installer and the stage script, signed like the other
   helpers.
2. macOS: both in `Flare.app/Contents/MacOS` (the jar in `Resources`), signed with `Role::Tool` like the sim host.
3. `stage-install` scripts on both OSes copy them; `release_files.rs` lists them.

## Housekeeping after Step 0

- Delete the `flare-spike` AVDs on the PC and the Mac (and the spike's 4 GB snapshot folders).
- The Mac SDK installed for the spike stays (Max may use it); `flare-spike` goes.
