# A live Android emulator beside a tab, on Windows and macOS (device panel, second source)

Status: draft for Max's approval (2026-11-22). Plan: `docs/superpowers/plans/2026-11-22-android-device.md`.

Research and spikes: `~/Documents/Claude/research/flare/2026-11-22-android-emulator.md` (spike numbers) and its folder
`2026-11-22-android-emulator/` (five detailed reports and the probe scripts).

Builds on:
- `docs/superpowers/specs/2026-11-19-device-panel-design.md` (the iOS device panel). This spec only says what
  Android adds or changes. Everything not named here works as in the iOS spec: the panel slot, persistence shape,
  takeover, touch indicators, the agent job, the tool set, the tree filter and refs, setup-state style, diagnostics.
- The device code on main after PRs 1, 2, 3, 4a, 4b, 5 and 6 of #248 (`crates/flare-device`,
  `crates/flare/src/device_*.rs`).

## Intent

Max's words (2026-11-22): plan and spec "how we'll build the android simulator, needs to work on both mac and pc. and
performance is the most important thing, just like for the ios simulator". He added that it "should also work on
windows though, but they might use different engines".

Why: the same reason as iOS. An agent that builds an Android app works blind today, or drives the emulator through
`adb shell input` and `screencap` (hundreds of ms each) while the person cannot see or touch it. A panel beside the
tab lets the person watch the app at full smoothness, use it, and take over.

Words used here (in addition to the iOS spec's):
- **Emulator**: Google's Android Emulator, which Flare downloads from Google into its own SDK folder (or the person's
  own SDK, if they have one).
- **AVD**: an Android Virtual Device, the emulator's saved device (name, image, size).
- **Emu host**: `flare-emu-host`, a small helper program Flare ships and starts. It is the only code that talks to
  the emulator (gRPC) and to adb.
- **Frame region**: a shared-memory region Flare owns, which the emulator writes each new frame into.
- **Agent helper**: `flare-agent.jar`, a small Java program Flare pushes into the running Android device to read the
  UI tree fast. It runs as the shell user, like `scrcpy-server`; nothing is installed in Android.

## The two engines

The Android emulator is the same program on both machines, with different engines under it. Flare's code above the
emu host is the same; only the last step (frames into the GPU) differs.

| | Windows (PC) | macOS (Mac) |
|---|---|---|
| Guest images | x86_64 | arm64-v8a |
| Hypervisor | WHPX (Windows Hypervisor Platform; works with Hyper-V, WSL2, VBS) | Hypervisor.framework |
| Emulator GPU | gfxstream on the host GPU (`-gpu host`) | gfxstream on the host GPU (`-gpu host`) |
| Frames to Flare | gRPC `streamScreenshot`, MMAP into Flare's frame region | same |
| Flare's renderer | wgpu 30 on DX12 | wgpu 30 on Metal |
| Region to GPU (Step 0 S3) | D3D12 heap on the mapped view + compute copy, no CPU copy | `newBufferWithBytesNoCopy` + compute copy, no CPU copy |
| Cold boot (spike, first boot) | 139 s | 23 s |
| Touch to photon, probe app drawing every frame (Step 0 S2, p50) | 51 ms | 45 ms |

## Goals

- A device panel beside a tab shows an Android emulator on **Windows and macOS**, in the same slot as the iOS
  device panel and the browser panel. On a Mac both kinds are offered; on Windows Android only.
- **A steady 60 fps**: at least 99% of the frames the guest composes are shown over a 15 s run with a moving screen,
  frame interval p99 under 25 ms, never more than 60 a second. Measured with the probe app (see Test plan).
- **No CPU copy of a frame in Flare** if spike S3 finds a working import on that OS; otherwise at most one copy,
  under 1.5 ms per frame at 1080x2400, measured, and a gate in the PR.
- The person taps, drags, scrolls and types in the panel. A tap shows in the panel within **50 ms p50** of the mouse
  press on both OSes, measured with the probe app (Step 0 S2: PC 51 ms, Mac 45 ms; most of it is the guest's own
  two-to-three-frame pipeline; the gate is checked again in Rust in PR A4, where Python's per-frame decode is gone; open
  decision 6).
- The agent uses the same `device_*` tools as for iOS, with Android meanings (package names, `.apk` files, Back and
  Recents buttons). `device_look` with the UI tree answers within **150 ms p50** (the iOS spec's order of size).
- Nothing appears on screen besides Flare's own window: no emulator window, no taskbar or Dock entry, no prompt.
  (Spike A5: 0 visible windows on Windows, no app registered on the Mac.)
- A crash or hang in the emulator or the emu host never takes Flare down.
- Everything stays general: panel kind `device`, tools `device_*`, the `DeviceSource` trait. No "android" or "emu" in
  a tool name, schema key or panel kind. (A target string such as `emu/Pixel_8` is data.)

## Non-goals

- Real Android phones over USB or Wi-Fi (the research names the route: scrcpy-style H.264 plus hardware decode; a
  later phase).
- Shipping the emulator or system images inside Flare's installer (Android SDK licence forbids redistribution).
  Flare downloads them from Google on the person's machine instead (see "Setup").
- Android Studio, Java or a separate SDK install. None is needed (Max, 2026-11-22: "we don't rely on things we
  install manually").
- Build, Gradle, logcat or test tools. The agent builds with Gradle in its shell.
- Showing the device in a remote tab or on another machine.
- Foldables' second screen, Wear, TV, Auto and XR images (the main display only, `display: 0`).
- Plugins (mods) opening or driving a device panel.

## Anti-goals (a failure even if it works)

- Any emulator window, taskbar button, Dock icon or permission prompt appears, even for a moment.
- The panel stutters or tears while the guest itself runs at 60.
- The person's computer slows down because the panel streams while hidden (the stream stops when the panel is not
  seen; see "Frames").
- Flare's window freezes while an emulator boots (139 s cold on the PC), the emu host restarts or adb is slow (rule 34).
- Flare boots an emulator nobody asked for, or shuts down one it did not boot (one started from Android Studio).
- Typing in the panel changes the computer's clipboard.
- An emulator Flare booted keeps running (5 GB of RAM) after Flare crashed and nobody uses it.
- Flare opens an unprotected gRPC port that any local program could use to drive the device.

## Findings that decide the design

Facts, with the spike or report that holds them:

1. **Frames.** The only public frame API is gRPC `streamScreenshot` (`emulator/lib/emulator_controller.proto` in the
   person's SDK). With `ImageTransport{MMAP, handle}` the emulator reads the composed frame back from the GPU once
   and writes it straight into a region the client owns; the `Image` reply carries only `seq`, `timestampUs`, size
   and rotation. The server sends at the guest's vsync, only on change, and does not pace or buffer (`frames.md`).
2. **Spike A1**: the MMAP stream keeps up with the guest on both machines, no `seq` gaps. Mac 60.1 fps, interval
   p99 18.8 ms, 10% client CPU. PC 54.5 fps while the guest itself made 55 (the scroll driver loads the x86 guest).
   Pixels in the gRPC message instead cost 150 to 170% CPU in the Python probe: out.
3. **The region is single-buffered** ("mmap can result in tearing"). The emulator overwrites it with the next frame.
   So Flare must take the frame out of the region (a GPU copy) before the next one lands, about 16 ms later.
4. **No GPU texture export** exists (no D3D handle, no IOSurface). A zero-copy GPU route would need a fork of
   gfxstream. Not done.
5. **Input.** `streamInputEvent` is one long-lived client stream for touch (up to 10 contacts, lift with pressure
   0), mouse, wheel and keys. Step 0 S2 with the probe app (an app drawing every frame): touch to photon p50 51 ms on
   the PC, 45 ms on the Mac. (The first spike's 7.6 and 61 ms used the system pointer overlay, which is drawn
   differently; do not use it as a measure.)
6. **UI tree.** `uiautomator dump` takes 2.0 to 2.2 s (spike A4) and fails on animated screens. A shell-uid
   `app_process` helper holding one `UiAutomation` connection is the fast route (`input-and-tree.md`: 10 to 40 ms
   estimated; spike S4 measures it).
7. **Auth.** Plain `-grpc <port>` opens the port to every local program (spike: "No gRPC protection active").
   `-grpc-use-token` puts a token in the discovery file; the client sends `authorization: Bearer <token>`. Android
   Studio does the same.
8. **Discovery.** A running emulator writes `pid_<pid>.ini` in `%LOCALAPPDATA%\Temp\avd\running` (Windows) or
   `~/Library/Caches/TemporaryItems/avd/running` (macOS) with `grpc.port`, `grpc.token`, `port.serial`, `avd.name`,
   `emulator.version`.
9. **Licence.** The emulator and system images come under the Android SDK licence, which forbids redistribution.
   Flare never ships them; it downloads them from Google on the person's machine after the person accepts Google's
   licence in Flare (see "Setup").
10. **Versions.** Mac MMAP was broken before 37.2.12 (issue 537802959). 37.2.13 fixed a GLES memory leak. 37.3.10
    fixed App Nap lag on the Mac and E-core throttling on Windows. The next-gen emulator (QEMU 11+, preview) has no
    documented gRPC or MMAP support yet.
11. **Flare today** (`flare-code.md`): `DeviceSource`, the agent job, indicators, tree filter and `FrameCap` are
    general. `Target` has only `Sim`; `Button` has no Back, Recents or Power; the gate, setup states and `Starter`
    are tied to Xcode; `device_frames.rs` is a stub off macOS; `flare-proc`'s guard is macOS only (Windows has the
    kill-on-close job pattern in three crates). Flare draws with DX12 on Windows; `windows` lacks the
    `Win32_Graphics_Direct3D12` feature.

## Builder's calls (Claude's, can be overruled)

1. **A helper process, like iOS.** `flare-emu-host` (a new `[[bin]]` in `crates/flare-device`, code under
   `src/emu/`) holds the gRPC client, adb and the agent helper. Flare talks to it with the same JSON-lines control
   protocol as the sim host (`wire.rs`, a new `Hello` kind). Reasons: the gRPC stack (tonic, prost, tokio) stays
   out of `flare.exe`; a hung or crashed gRPC call cannot block or crash Flare; the client supervisor, restarts and
   still-mode fallback in `client.rs` are reused.
2. **Flare owns the frame region**, not the emu host. Flare makes a page-aligned file-backed mapping per device,
   sized for the device's largest screen, maps it once, and sends its `file:///` URL to the emu host, which passes it
   to the emulator. **Where**: emulator 37.3.10 accepts a region file only inside the emulator's discovery folder,
   `/tmp` or `/tmp/android-<user>` (it answers PERMISSION_DENIED for anything else; Step 0 S7, 2026-11-22; 37.2.13
   did not check). So the file goes in a `flare` folder inside the discovery folder:
   `~/Library/Caches/TemporaryItems/avd/running/flare/<pid>-<n>.rgba` (macOS, tested) and
   `%LOCALAPPDATA%\Temp\avd\running\flare\<pid>-<n>.rgba` (Windows, tested; Windows 37.3.10 does not enforce the rule, Flare keeps one place on both), user-only
   permissions, deleted on detach and when Flare starts. Named shared memory is not used.
3. **Frame records travel on the emu host's stdout** as one small JSON line each (`{"frame":{target,seq,t_us,w,h,
   rotation}}`), 60 a second at most. No mach port, no extra descriptor: the same channel on both OSes.
4. **The GPU copy happens at once, off the UI thread.** A frame thread in Flare reads each frame record and right
   away records a GPU copy of the region into the next free slot of a 3-texture ring (`RING_SLOTS`), with the Y flip
   if the rows are bottom-up (spike S1 says), then submits. wgpu's `Queue` is safe to use from another thread. The
   render notifier then shows the newest finished slot in `BeforeRendering`, as for iOS (`DeviceFrames`). This keeps
   the copy well inside the 16 ms before the emulator overwrites the region (finding 3).
5. **The copy itself** (chosen by Step 0 S3, 2026-11-22; no CPU copy on either OS):
   - Windows: open the mapped view as a D3D12 heap (`ID3D12Device3::OpenExistingHeapFromAddress`; the
     `FromFileMapping` form refuses a file-backed mapping), place a buffer on it (`buffer_from_raw` +
     `create_buffer_from_hal`), and copy it into the ring texture with a small compute pass (1080-pixel rows are
     4320 bytes, which breaks the 256-byte row rule of `copy_buffer_to_texture`). 0.07 ms of CPU and 0.7 ms on the
     GPU per frame (S3, RTX 5090).
   - macOS: `newBufferWithBytesNoCopy` on the mapping (unified memory), wrapped the same way, the same compute copy.
     0.02 ms of CPU and 0.3 ms on the GPU per frame (S3, M6).
   - Fallback on both: `Queue::write_texture` from the mapped pointer (one CPU copy, 0.4 ms on the PC and 0.7 ms on
     the Mac per frame), logged once, if the import fails on a machine.
   Both feed the same ring and the same `Image::try_from(texture)` path the iOS panel uses.
6. **The stream runs only while the panel is seen.** When the panel closes, its tab goes behind, or the window is
   minimised or occluded, Flare cancels `streamScreenshot` (the emulator then reads nothing back). It starts again at
   once when seen. The emulator keeps running, so the agent keeps working: `device_look` takes one `getScreenshot`.
7. **Coordinates for the agent are dp** (pixels / (dpi / 160)), the Android match for iOS points. `device_look`
   returns its PNG at dp size (1080x2400 at 420 dpi gives 411x914), so its pixels are tap coordinates. Only the emu
   host converts dp to pixels.
8. **Keys from the person** go as USB HID usages (`codeType: Usb`, the existing `keymap.rs`), key down and key up
   separately. Text the keymap cannot type goes through the agent helper (`ACTION_SET_TEXT`), else the guest
   clipboard (gRPC `setClipboard`, which is the guest's own clipboard) plus paste. The computer's clipboard is never
   touched.
9. **Buttons**: `Button` gains `Back`, `Recents` and `Power`; Android maps Home, Back, Recents and Power to gRPC
   `key` names (`GoHome`, `GoBack`, `AppSwitch`, `Power`), volume to evdev 115 and 114, rotation to
   `setPhysicalModel(ROTATION)`. `Shake` and `Lock` are iOS only; each source lists the buttons it has and
   `device_press` refuses the others with `unavailable`.
10. **Targets** are `emu/<avd name>` (the AVD name is stable; the adb serial changes per boot). Valid AVD names are
    `[A-Za-z0-9._-]`, at most 64.
11. **Flare boots with**: `emulator -avd <name> -no-window -no-audio -no-boot-anim -gpu host -grpc <free port>
    -grpc-use-token -idle-grpc-timeout 300 -port <free even port 5554..5682>`. Quick Boot stays as the AVD has it,
    so a second boot is fast. `-no-window` picks the headless engine (spike A5: nothing on screen).
12. **A watchdog instead of a job object for the emulator.** An emulator Flare booted is not put in a kill-on-close
    job (a hard kill can damage the AVD's data). It gets `-idle-grpc-timeout 300`; the emu host calls `getStatus`
    every 60 s while Flare runs. If Flare and the emu host die, the emulator shuts itself down cleanly 5 minutes later.
    The emu host itself is guarded (macOS: `flare_proc::guard`; Windows: a new kill-on-close job in `flare-proc`,
    lifted from `flare-dictation/src/job.rs`).
13. **Attach to an emulator Flare did not boot** (started from Android Studio) when its discovery file has a gRPC
    port and token: shown and driven like any other, never shut down by Flare. Without a token (an emulator started
    with plain `-grpc` or none) Flare shows "This emulator was started without access for other apps. Start it from
    Flare." and nothing else.
14. **The agent helper jar is built from source in this repo** (`tools/android-agent/`, Java, about 500 lines,
    compiled with `javac` and the SDK's `d8` by a script, the output checked in under `crates/flare-device/assets/`
    with its source hash, and checked in CI). It is Flare's own code under Flare's licence. It is pushed on attach
    (`adb push` to `/data/local/tmp/flare-agent-<version>.jar`), started with `app_process`, and reached through
    `adb forward tcp:0 localabstract:flare-agent-<pid>`. If it fails, the tree falls back to `uiautomator dump`
    with a "slow" mark in `device_state`.
15. **Rule numbers** start at 1360 (see the plan).
16. **The fake source** gains an Android preset (`FakeSource::android()`: a Pixel-sized screen, Android roles), so
    the e2e scenarios run for both kinds on the Windows and Mac e2e jobs.

## How it works

### Parts

```
 Flare (UI thread + frame thread)           flare-emu-host (one per Flare, guarded)        emulator (person's SDK)
 --------------------------------           ---------------------------------------        -----------------------
 DeviceCtl --- control: JSON lines ----->   gRPC client (tonic) ----- localhost, token --> EmulatorController
           <-- answers, events, frame -----  streamScreenshot(MMAP url) ----------------->  writes each frame
               records (JSON lines)          streamInputEvent (held) ---------------------->  into the region
 frame region (file mapping, Flare owns) <=============================== written by the emulator
 frame thread: on each record, GPU copy region -> ring slot; notifier shows newest slot
                                             adb: install, am start, push + forward ------> flare-agent.jar (shell uid)
                                             agent helper socket: tree, set text, front app
```

- `DeviceSource` is unchanged in shape. A new `EmuSource` (Flare side, `crates/flare-device/src/emu_client.rs`)
  starts and talks to the emu host, like `SimSource`. Its frame events are a new `SourceEvent::Region { target,
  seq, t_us, size_px, rotation }` (the region is already mapped), beside the iOS `Frames`/`Frame` and the CPU
  `Picture`.
- One emu host per Flare, started on the first Android panel, stopped two minutes after the last one closes. It can
  hold several emulators (one per tab).
- The emu host answers `hello {protocol, version, kind: "emu", sdk_path, emulator_version, adb_version,
  accel, agent_jar}` first.

### Setup: Flare installs what it needs (Max, 2026-11-22)

Max's words: "it's important that when we do this we don't rely on things we install manually ... I assume we can
package everything with Flare or as an install you can do". iOS needs Xcode (Apple allows no other way); Android
needs nothing the person installs by hand.

What Flare ships in its own installer: `flare-emu-host` and `flare-agent.jar` (Flare's own code). What Flare
downloads from Google on the person's machine, after one click: the emulator, platform-tools (adb) and one system
image. The licence forbids Flare from redistributing these, so they always come from Google's servers, never from
Flare's.

- **Flare's own SDK folder**: `%LOCALAPPDATA%\Flare\android-sdk` (Windows) or
  `~/Library/Application Support/Flare/android-sdk` (macOS). If the person already has an SDK (`ANDROID_HOME`,
  `ANDROID_SDK_ROOT`, `%LOCALAPPDATA%\Android\Sdk`, `~/Library/Android/sdk`) with an emulator at or above the floor,
  Flare uses it and also lists its AVDs; it never changes or updates the person's own SDK.
- **No Java, no `sdkmanager`.** The emu host reads Google's public package lists itself
  (`https://dl.google.com/android/repository/repository2-3.xml` for `emulator` and `platform-tools`,
  `.../sys-img/google_apis/sys-img2-3.xml` for images; checked 2026-11-22: plain zip URLs such as
  `emulator-windows_x64-16916650.zip`, `emulator-darwin_aarch64-16916650.zip`, `x86_64-36_r07.zip`,
  `arm64-v8a-36_r07.zip`, each with a size and SHA-1). It downloads on a worker, checks the checksum, unpacks into the
  SDK folder with the `package.xml` the SDK expects, and records the accepted licence the way the SDK does
  (`licenses/android-sdk-license` with the licence text's hash).
- **Licence**: before the first download the panel shows Google's licence text, taken from the package list (the
  `android-sdk-license` element), with [Accept and set up] and [Cancel]. Nothing is downloaded before that click.
- **Size**: about 2.5 GB to download and about 8 GB on disk once unpacked with one device (Step 0 S7 on the Mac:
  emulator 421 MB, platform-tools 17 MB, API 37 arm64 image 2.1 GB, downloaded and checked in 48 s; 8.0 GB on disk
  after the first boot). The licence screen says the disk size, and Flare checks free space first.
- **First boot**: a new device spends its first minute or two compiling apps in the background, and draws only
  about 36 fps meanwhile (S7). Flare shows "Preparing <name>..." after the first boot and runs
  `pm bg-dexopt-job` once before it calls the device ready. The panel shows
  one progress bar with MB and a [Cancel]; a cancelled or broken download leaves nothing half-unpacked (unpack to a
  temporary folder, then rename).
- **The device**: Flare writes one AVD itself, "Flare Pixel" (an `avd` folder beside Flare's SDK folder,
  `ANDROID_AVD_HOME` set for its emulators): a `config.ini` with the Pixel 8 screen (1080x2400, 420 dpi), 4 GB RAM,
  4 cores. No `avdmanager`. The person can add more from the chooser ([New device]: phone sizes only in v1).
- **Image choice**: the newest stable API `google_apis` image for the host CPU (x86_64 on Windows, arm64-v8a on
  Apple silicon), not a Play Store image (those need a Google sign-in in the device and have more licence limits).
- **Updates**: when the emulator in Flare's folder is below the floor, or Google lists a newer stable one, the
  chooser shows [Update Android emulator] (one click, same download path). Flare never updates while a device runs.
- **Windows hypervisor**: if `emulator -accel-check` says WHPX is off, the panel says "Android needs Windows
  Hypervisor Platform. Turning it on needs a restart." [Turn on] runs
  `dism /online /enable-feature /featurename:HypervisorPlatform /all /norestart` elevated, which shows Windows' own
  UAC prompt (the person's click), then says "Restart Windows to finish." This is the only step outside Flare, and
  only on PCs that do not have it yet.

The emu host lists AVDs (Flare's own plus the person's), reads the running ones from the discovery folder, and on
Windows runs `emulator -accel-check` once per Flare session (on a worker).

Panel states (each one line, at most one button), in addition to the iOS spec's live, still and booting:
1. **Not set up**: "Android devices need Google's Android emulator (about 2 GB, from Google)." [Set up]
2. **Licence**: Google's licence text, [Accept and set up], [Cancel].
3. **Downloading**: progress, MB, [Cancel].
4. **No hypervisor (Windows)**: the line and [Turn on] above; then "Restart Windows to finish."
5. **Choose a device**: one list with both kinds on a Mac (iPhones, then Android), Android only on Windows;
   running ones marked; [New device] and, when due, [Update Android emulator].
6. **Started outside Flare without access**: the line in builder's call 13.

### Boot, attach, shut down

- Boot: the command in builder's call 11, on a worker. The emu host waits for the discovery file of its child's pid,
  connects with the token, then waits for `streamNotification` `BootCompletedNotification` (or `getStatus().booted`),
  up to 300 s (the PC's first cold boot took 139 s). The panel shows "Starting <name>..." with seconds.
- Attach: open the region stream and the held input stream, push and start the agent helper, read the screen size
  and dpi (`getDisplayConfigurations`).
- Shut down (only emulators Flare booted, the iOS spec's `booted.json` rule): gRPC `setVmState(SHUTDOWN)`; at quit a
  detached `adb -s <serial> emu kill` so quit never waits. The setting `flare.device.shutdown_on_quit` covers both
  kinds.
- An emulator that dies or is closed outside Flare: the iOS spec's state 9 ("<name> was shut down. [Boot again]").

### Frames

In the emu host, per attached emulator:
- `streamScreenshot{format: RGBA8888, display: 0, transport: {MMAP, handle: <Flare's file URL>}}`. For each reply it
  writes one frame record line, applying the existing `FrameCap` (60 a second, newest wins, a lone frame never
  waits). `seq` gaps are counted for diagnostics.
- A reply with 0x0 size (display off) becomes `State(Asleep)`; the panel shows the last frame dimmed.
- A changed size or rotation in a reply (rotation, a resizable AVD) re-sizes the ring on the Flare side.
- `pause {target}` cancels the stream; `resume` opens it again (builder's call 6). A pause never stops input, the
  tree, `snapshot` or the "changed" counter (which the emu host keeps from the stream while live, and from the agent
  helper's window events while paused).

In Flare:
- The frame thread does builder's calls 4 and 5. Slot ownership is the existing `Ring` (a slot is rewritten only
  after `on_submitted_work_done` for the frame that sampled it).
- If the copy of frame N has not been submitted when record N+1 arrives, frame N is dropped (counted). Flare never
  queues frames.
- Fallbacks, in order: (1) `write_texture` if the zero-copy import fails on this machine, logged once; (2) the CPU
  path of the iOS spec (copy the region into a `SharedPixelBuffer`, at most 30 fps, "slow" mark) on the software
  renderer; (3) still pictures from `getScreenshot` at most 3 fps if the stream cannot be opened (for example the
  next-gen emulator lacks MMAP), with the still badge and the reason.

### Input

- The person: the existing `device-box` TouchArea and `device-keys` FocusScope. `DeviceCtl` maps panel points to
  device pixels (letterboxed) and sends `touch {phase, x, y}`; the emu host sends `TouchEvent` on the held stream
  with `identifier` 0 (and 1 for the Alt-drag pinch) and pressure 0 to lift. On panel close, focus loss and stream
  reset the emu host lifts every open contact.
- Wheel and trackpad: a short touch drag at the pointer, as for iOS. (gRPC `WheelEvent` would scroll only views
  that take mouse wheel; a drag scrolls everything a finger does.)
- Header buttons for Android: Back, Home, Recents, and a menu with Power, Volume up and down, Rotate left and right,
  Shut down, Choose device.
- Takeover: unchanged (`person_input(tab)`).

### UI tree and agent helper

- `flare-agent.jar` (builder's call 14) answers `tree`, `set_text {ref|focused, text}`, `front_app` and pushes
  window-change events. The tree: every window from `getWindows()` with `FLAG_RETRIEVE_INTERACTIVE_WINDOWS`, nodes as
  `{class, text, desc, hint, res_id, bounds_px, clickable, scrollable, editable, checked, focused, enabled}`. It does
  not wait for idle by default; `device_look` takes what is on screen now.
- The emu host converts bounds to dp and maps classes to the tree model's roles (`Button`, `ImageButton` and
  clickable nodes: button; `EditText`: text field; and so on), so `tree.rs` filters and numbers refs the same way
  for both kinds. Labels are untrusted and fenced as for iOS.
- Install and launch: `adb install -r -t <apk>` (the `.apk` must be inside the tab's folder, symlinks resolved, and
  name a package); launch with `cmd package resolve-activity --brief` then `am start -W -n`. `device_launch` takes a
  package name.

### The agent

The 12 `device_*` tools stay. Changes:
- Descriptions name both kinds without platform words: "a phone simulator or emulator".
- `device_list` returns `{target, name, kind: "iphone" | "android", os, state}`.
- `device_launch` takes `app`: an iOS bundle id or an Android package name.
- `device_install` takes `path`: an `.app` folder (iOS) or an `.apk` file (Android).
- `device_press` lists the buttons of the device in the caller's panel; others answer `unavailable`.
- Capabilities: Flare lists `device.*` when **any** source is ready: Apple silicon with Xcode, or an Android SDK with
  an emulator at or above the floor and at least one AVD. So on Windows the tools now appear where Android works.
- Remote and hidden sessions: `unavailable`, as for iOS.

### Persistence

`panels.json` keeps `{kind: "device", target: "emu/<avd>", open, width}`. Restore never boots. On a machine without
the AVD the panel shows "No virtual device named <avd> on this computer." with [Choose device], and keeps the entry
(moving `panels.json` between PC and Mac loses nothing).

## Versions and failure modes

| What happens | What Flare does |
|---|---|
| Not set up, or no WHPX | Setup states 1 to 4; tools answer `unavailable` with the same line (the agent cannot accept the licence or start the download). |
| Download fails or the checksum does not match | Nothing is unpacked; "The download failed: <why>." [Try again]. |
| Emulator below the floor | [Update Android emulator]; a device on an old emulator is not streamed (MMAP crashes old Mac emulators). |
| MMAP stream refused or writes nothing for 2 s while the guest is awake | Fallback: frames in the gRPC message, CPU path, "slow" mark; then still pictures if that fails too. Logged with the emulator version. |
| Next-gen emulator engine without gRPC | Still pictures through `adb exec-out screencap -p` at most 2 fps, input through `adb shell input` with the still badge and the reason. |
| Boot over 300 s | "Still starting..." and the agent call answers `busy`; the boot carries on. |
| The emu host crashes | Restarted, reattached (max 3 in 5 minutes, then still mode for the session). The emulators keep running. |
| Flare crashes | The guard kills the emu host; emulators Flare booted stop by themselves after 5 idle minutes. |
| Two Flares (dev and release) attach to one emulator | Both stream and both drive it; not prevented in v1 (as for iOS). |
| Android Studio and Flare show the same emulator | Both work (gRPC allows several clients); Flare never shuts it down. |
| The agent helper fails to start (new Android version) | The tree falls back to `uiautomator dump` (about 2 s) with "slow tree" in `device_state`. |
| Rotation | The emulator sends rotated frames (`rotation` in each reply); the ring re-sizes; touch maps follow. |
| Guest display asleep | Last frame dimmed; a tap or a key wakes it (Power). |

## Diagnostics

- The emu host logs to `<profile>/logs/emu-host.log` (rotating like `sim-host.log`): hello facts, each boot and
  attach with the emulator command line (no token), each fallback and reason, and once a minute while live: frames
  received, shown, dropped by Flare, `seq` gaps, copy time p50 and p99.
- `device_state` reports the same, plus the frame path (`zero-copy`, `upload`, `cpu`, `still`) and the tree path
  (`helper`, `dump`).
- A `--bench <avd> <seconds>` mode of `flare-emu-host` boots a device made for the run, runs the probe app, prints
  the A1 to A3 numbers as JSON, and deletes the device.

## Security notes

- **Agent, before**: iOS simulators on a Mac. **After**: also Android emulators on Mac and Windows: boot, look,
  drive, install `.apk` files from its tab's folder, launch apps, read the UI tree. An emulator is sandboxed from the
  computer; an installed app runs inside it.
- **gRPC**: Flare starts emulators only with `-grpc-use-token`. The token is read from the discovery file by the emu
  host, never logged, never sent to the agent. Flare never starts an emulator with plain `-grpc` (finding 7).
- **The frame region** is a file under the profile, created with user-only permissions, deleted on detach and at
  start. Another process of the same user could read it, as it can read the emulator itself.
- **The agent helper** runs as Android's shell user inside the emulator only (it holds `INJECT_EVENTS` and reads
  every window). It listens on an abstract socket inside the device, reached only through `adb forward` to
  localhost. It is never pushed to a real phone in v1.
- **UI-tree text** is untrusted: cleaned and fenced, as for iOS.
- **Plugins**: no access, as for iOS.

## Test plan

All headless. Real emulators only in spikes, the bench and the opt-in real scenario, always on an AVD made for the
run (`avdmanager create avd -n flare-test-<n>`) and deleted after, never one of Max's (`uc-api29`, `uc-api37`).

- **Pure (flare-device)**: `emu/` targets and AVD-name checks; Android buttons per source; dp maps in four
  rotations; class-to-role mapping; the frame-record parser; discovery-file parser (fixtures from the spike); SDK and
  version detection (floors per OS); the boot command line (always `-grpc-use-token`, never plain `-grpc`).
- **GPU import (no window)**: per OS, a test makes its own wgpu device, a file-backed region with a known pattern,
  runs the chosen import and the GPU copy with the Y flip, reads back and checks pixels; and checks
  `Image::try_from` accepts the ring texture. Windows runs on the Windows CI job (WARP adapter if no GPU), macOS on
  the Mac job.
- **Emu host without an emulator**: `--self-test` finds the SDK (or says why not), parses the protos it was built
  with, checks the agent jar's hash, and exits. Runs in CI on both OSes.
- **App (`ui_tests`, `appctl/tests`)**: the Android header buttons, the setup states 1 to 6, the dp letterbox map,
  takeover; with `FakeSource::android()`.
- **e2e (fake)**: `devicePanel`, `deviceAgent` and `mcpDevice` gain an Android variant, run on the Windows and Mac
  e2e jobs.
- **e2e (real, opt-in)** `deviceRealAndroid`: on the GitHub runners (they ship an Android SDK) only when its full
  name is the filter: creates an AVD, boots it headless, installs the probe app, checks 891 of 900 frames over 15 s,
  touch to photon under 50 ms p50, no visible window, deletes the AVD.
- **Probe app** (`tools/android-agent/probe/`, never shipped): one activity that paints a frame counter as a barcode
  every vsync and the time of the last touch, for the gates above.

## Open decisions for Max (each with a recommendation)

1. **Architecture**: a helper process `flare-emu-host` with the gRPC client (recommended, builder's call 1), or the
   gRPC client inside Flare's own process (one hop less, but tonic and tokio in `flare.exe`).
2. **UI tree in v1**: the agent helper jar, built from Java source in the repo (recommended: about 50x faster than
   `uiautomator dump`, which takes 2 s), or `uiautomator dump` only in v1 and the jar later.
3. **Emulators started outside Flare**: attach and drive them when they allow it, never shut them down (recommended),
   or only show emulators Flare booted.
4. **SDK setup** (answered by Max later the same day: Flare installs everything itself): v1 detects the SDK and points to Android Studio, or v1 also offers a one-click setup
   that runs Google's `sdkmanager` (the person accepts Google's licence in Flare).
5. **Crash cleanup**: `-idle-grpc-timeout 300` so an emulator Flare booted stops 5 minutes after Flare dies
   (recommended), or leave it running like the iOS simulators.
6. **The Mac touch gate**: if spike S2 cannot bring the Mac under 50 ms p50, ship with the Mac at its measured
   latency and a note (recommended, the picture is still 60 fps), or hold the Mac until it passes.
7. **Mockup step** for the Android header buttons, the device chooser with both kinds, and the new setup states
   (recommended, with impeccable as for PR 4a), or reuse the iOS layout as is.
8. **Next-gen emulator**: still-picture fallback only (recommended) until Google documents its gRPC, or block it.

### Decisions taken (2026-11-22, Max: "Approve, all recommendations")

| # | Chosen |
|---|---|
| 1 | Helper process `flare-emu-host`. |
| 2 | Agent helper jar in v1; `uiautomator dump` only as the fallback. |
| 3 | Attach to emulators started outside Flare when they allow it; never shut them down. |
| 4 | **Changed by Max later on 2026-11-22**: Flare installs everything itself (its own SDK folder, downloads from Google after the licence click, no Java, its own AVD). See "Setup". |
| 5 | `-idle-grpc-timeout 300` watchdog for emulators Flare booted. |
| 6 | If S2 cannot fix it, the Mac ships at its measured touch latency with a note. |
| 7 | Mockup step before PR A4. |
| 8 | Next-gen emulator: still-picture fallback only. |

## Open checks for Max's by-hand test

1. On a PC or Mac with no Android SDK: open a device panel, [Set up], accept the licence. It downloads, makes
   "Flare Pixel" and boots it with nothing else on screen.
2. Scroll Settings fast on the PC and on the Mac. It looks as smooth as Android Studio's emulator, with the right
   colours.
3. Tap, drag, type (also æøå), Back, Home, Recents from the header.
4. Ask Claude to open Settings and search for "Wi-Fi". Watch the dot, ripple, trail and badge.
5. Touch the panel while Claude swipes: Claude stops and says you took over.
6. Start an emulator from Android Studio: Flare lists it and can show it; quitting Flare leaves it running.
7. Quit Flare with an emulator it booted: it shuts down. Kill Flare from Task Manager: the emulator stops by itself
   about 5 minutes later.
