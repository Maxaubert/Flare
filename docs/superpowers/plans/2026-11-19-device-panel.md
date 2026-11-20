# Device panel (live iPhone simulator beside a tab, issue #248): implementation plan (2026-11-19)

Spec: `docs/superpowers/specs/2026-11-19-device-panel-design.md`.
Status: draft for Max's approval, together with the spec. Step 0 done 2026-11-19 (every gate passed, see "Open
checks"); no PR started.

One gating step (spikes, no PR), then seven code PRs. Each builds and passes its own tests. Merge order:
**1, 2, 6, 3, 4a, 4b, 5** (PR numbers are names, not the order).
- Step 0 (spikes) gates everything. S2 and S3 must pass before PR 2 starts; S1 before PR 3; S5 decides the Rotate
  parts of PR 3 and PR 4a. If S1 fails, PR 3 switches to the AXe v2.0.4 fallback (open decision 2); if S2 fails,
  PR 2 takes open decision 3b.
- PR 1 (pure model) and PR 2 (sim host frames) ship alone; nothing is wired, so users see no change.
- PR 6 (bundle and sign the sim host) ships alone right after PR 2: the bundle carries a helper nothing starts yet,
  and the notarized build proves S3 on the real release path early, before any UI depends on it.
- PR 3 (sim host input and UI tree) ships alone, still unwired.
- PR 4a (the panel with the fake source: kind, setup states, persistence, person input, CPU path) ships with no
  menu entry on a real Mac (only `--e2e` and a test-only switch show it), so it can merge alone.
- PR 4b (the real source and the zero-copy GPU path, the menu entry, quit shutdown) is the first visible change.
- PR 5 (agent tools and indicators) ships alone after PR 4b.
- PR 4a has a mockup step first (open decision 10).

## Rule numbers

On origin/main 30a4e7b8 the last rule is 1286. 1287 to 1301 are taken or reserved by open PRs #280, #283, #287 and
#289; the opener plan (`plans/2026-11-19-remote-session-on-opener.md`) claims 1302 to 1309. This plan takes **1310 to
1322**. Check `docs/regression-rules.md` and open PRs again before each PR and renumber if another PR took them.
New rules go in a section `## Device panel (2026-11-19)` at the end of the file.

| Rule | Says | PR |
|---|---|---|
| 1310 | Flare ships its own private-API code for the simulator, only in `flare-sim-host`; Flare's own process never loads CoreSimulator or SimulatorKit. Every private class, selector and symbol is checked before use and every call is wrapped against exceptions and given a 5 s limit. Reverses the 2026-11-16 "no private-API code of our own" (issue #248). | 2 |
| 1311 | The device picture reaches the screen with no CPU copy: an RGBA IOSurface ring (`RING_SLOTS`) blitted by the sim host, wrapped once as wgpu textures, shown as a Slint image in `BeforeRendering`. At most 60 frames a second (15 ms spacing, newest wins, a lone frame never waits). A slot is never rewritten while Flare samples it: it is freed only from `on_submitted_work_done` registered in `AfterRendering`. | 2, 4b |
| 1312 | The sim host is guarded: it dies with Flare, it is restarted at most 3 times in 5 minutes, then the panel shows still pictures (`simctl` screenshots, at most 3 fps) with a still badge and the reason in one line. Flare never crashes or freezes with it. | 2, 4b |
| 1313 | Input uses DTUHID from CoreSimulator 1155.4 and legacy Indigo below it, never both in one boot. One connection per device is held for the session. Points are device points in the current orientation; only the sim host converts to portrait ratios. | 3 |
| 1314 | Typing never touches the Mac's clipboard: keymap text goes as HID key usages; other text through the simulator's own pasteboard (`simctl pbcopy`) and Cmd-V. | 3 |
| 1315 | UI-tree text is untrusted: cleaned, fenced in the page-content markers, filtered (zero-size and unlabelled full-screen wrappers dropped), at most 300 refs. | 3, 5 |
| 1316 | A device panel is kind `device` (Slint kind 5), target `sim/<udid>`, saved in `panels.json`; restore never boots a device, it shows Boot; `target_ok` checks only the target's shape. Older Flare drops the unknown kind (rule 40). | 1, 4a |
| 1317 (now 1323, #293 took 1317) | The device panel never opens Simulator.app, Device Hub or a permission prompt; simulators boot headless; Flare never boots a device no window of its own shows (remote and hidden sessions answer `unavailable`). | 4b, 5 |
| 1318 | Setup states each say one line with at most one button: no Xcode, Xcode not set up, no runtime, choose device, booting, live, still, shut down outside Flare. Not offered on Windows or Intel Macs (only the fake source under `--e2e`). | 4a |
| 1319 | Flare shuts down at quit only the simulators it booted (setting `flare.device.shutdown_on_quit`, default on), remembered across a crash in `device/booted.json`; quit never waits for it (a detached `simctl shutdown`). | 4b |
| 1320 | The person's press, key or scroll in the device panel ends a running agent call with `interrupted`; not while the splitter is held. | 4a, 5 |
| 1321 | The agent's touches show as a finger dot (agent cursor glide timing), a tap ripple, a swipe trail and a keyboard badge, plus the edge glow; drawn by Slint over the picture, never in `device_look`; hidden at once on takeover, close, tab behind or minimise. | 5 |
| 1322 | `device_*` tools act only on the caller's own tab's panel; `device_shutdown` only on a device Flare booted; `device_install` only from the tab's folder or DerivedData; plugins have no device access (no permission maps to `Namespace::Device`). | 5 |

Existing rules to respect and cite: 34 (blocking work on workers), 40 and 41 (unknown stored values, stores fail
soft), 130 (texture limits), 207 (a mod's calls are checked against its live grant), 229 (cancel), 240 (a panel keeps
its own view), 254 (`browser_point`), 1053 and 1054 (guarded helpers), 1105 (Mac e2e gates), 1117 (minimised and
occluded window on macOS), 1138 (signed code in the bundle), the agent cursor rules from the 2026-10-26 spec.

## Shared decisions (every PR)

- No ctl protocol bump: `device.*` is a new namespace, additive. `panel.state` gains a `device` kind.
- Grant change: `Namespace::Device`, open to `Grant::Agent` only, mapped to no `Permission` (PR 5).
- Blocking work on workers (rule 34): every `simctl` call, every sim host request, PNG encoding. UI thread only
  reads results via `UiMsg::Device` with generation numbers. Mark `sync-ok` where a sync call is proven cheap.
- Tests headless only. `FakeSource` in unit, ui_tests and e2e. A real simulator only in the opt-in `deviceReal`
  scenario (GitHub macOS runner), the spikes and the by-hand bench, always on a device made for the run
  (`simctl create`) and deleted after, never one of Max's. Nothing takes Max's screen: no step opens Simulator.app,
  Xcode, a browser or a staged Flare on it. Max's by-hand checks are his to run; if Claude has to start a staged
  Flare on screen for one, it asks first with AskUserQuestion (once per session).
- New workspace deps, exact pins at what `Cargo.lock` already has: `objc2-metal =0.3.2`, `objc2-io-surface =0.3.2`
  (PR 2). wgpu-hal is reached as `wgpu::hal` (no new dependency). Each gets a README "Dependencies" bullet and a
  `THIRD-PARTY-NOTICES.md` entry in the same PR. `cargo deny check` must pass. idb is a reference, not a
  dependency; any code ported from idb carries its MIT notice in the file header and in `THIRD-PARTY-NOTICES.md`.
- Hard gates per PR: `cargo nextest run --workspace`, `cargo fmt --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo deny check`, `tools/release/check-no-emdash.sh`.
- Pure-code PRs with no wiring use `#![cfg_attr(not(test), allow(dead_code))]`.
- macOS code under `crates/flare-device/src/macos/`, `#[cfg(target_os = "macos")]`; the crate builds empty of it on
  Windows. The `flare-sim-host` bin builds on Windows too (Cargo builds every bin of a workspace member) as a stub
  that prints "macOS only" and exits 2; its macOS deps sit under `[target.'cfg(target_os = "macos")'.dependencies]`.
- Rust worktrees share one `CARGO_TARGET_DIR` (`<repo>/.claude/targets/shared`); touch changed files before a build.
- Each PR: build on the Mac and stage the build for Max (never install over his running `Flare.app`), confirm the
  version, then the merge question.

---

## Step 0: gating spikes (no PR, Mac mini, headless)

Results go to `~/Documents/Claude/research/flare/2026-11-19-ios-simulator-input.md` (with the input read-through
written up from the workflow findings) and a short summary under "Open checks" below.

**Files (scratch only, in the research probe folder).** `hid.m` or `hid.rs` (DTUHID tap and type), `rdv` (mach
rendezvous parent and child), `rot.m` (orientation), the probe app extended to paint the last touch's frame number
and position. Every spike runs on a simulator made for it (`simctl create`, deleted after) and opens no window.

**Steps.**
1. **S1, input latency (the gate).** From a long-running probe: open the DTUHID digitizer connection, liveness
   barrier, then 100 taps at random points and 20 swipes against the probe app; the app paints, per frame, the
   frame number of the first touch it saw and the touch point. Measure press-to-painted-frame latency, landing
   accuracy (within 1 pt), and key input (type 100 characters, count). Also time cold connect after a fresh boot,
   and try one two-finger pinch (a second contact in `IndigoDigitizerEvent`).
2. **S2, ring handover, no window.** A parent process checks in a per-pid mach service and spawns the child through
   the same double fork as `guard_command` (or with `guard_command` itself from a tiny Rust test binary), which sends
   a receive right and three IOSurface mach ports; the parent maps them, checks the audit-token pid, wraps one as
   `MTLTexture` on its own wgpu 30 Metal device (`wgpu::hal::metal::Device::texture_from_raw` +
   `create_texture_from_hal`), copies it to a buffer and checks the child's test pattern, and checks `Image::try_from`
   accepts the texture. Then 600 frame records one way and 600 slot-done records back over the channel, timed. No
   Slint window is made.
3. **S3, signing.** Sign the S1 and S2 probes as tools with the hardened runtime and no entitlements, twice:
   Developer ID (team WV7BY8CUM8) and ad hoc (`-`, as dev builds are). Start each from a signed parent (the S2 parent
   signed like Flare's main exe), so macOS takes the parent as responsible. Check frames, input, the UI tree and
   `simctl` all still work, and that no TCC prompt appears. If library validation refuses, note which entitlement is
   needed (`disable-library-validation`), which changes PR 6's pack.sh check and `sign.rs`.
4. **S4, UI tree speed.** `AXPTranslator` over `sendAccessibilityRequestAsync` on Settings: elements and ms. No
   Accessibility prompt from the signed S3 build.
5. **S5, rotation.** Find a headless way to rotate the device and to read its orientation (idb, serve-sim, Baguette
   and AXe v2.0.4 sources first), and whether `framebufferSurface` changes size or stays portrait when it turns.
6. **S6, colour.** Read the framebuffer IOSurface's colour space (`IOSurfaceCopyValue` `kIOSurfaceColorSpace`), for
   by-hand check 2.

**Done when.** Each spike has numbers in the research file
(`~/Documents/Claude/research/flare/2026-11-19-ios-simulator-input.md`, linked from the spec). S1 passes if tap p50 is
at most 50 ms, 100/100 land, 100/100 characters arrive. S2 passes if the pattern reads back, the pid check refuses a
wrong pid, and records round-trip under 1 ms p99. S3 passes if both signings work with no prompt. S5 has an answer
(a way, or none). Max answers open decisions 2, 3b and 11 with the results; S2 and S3 before PR 2 starts, S1 and S5
before PR 3 starts.

Done 2026-11-19: every gate passed; Max delegated the open decisions (2, 3b and 11 included), taken under "Open checks"
below.

---

## PR 1: The pure model (no wiring)

Branch `feat/device-panel-1`. Ships alone.

**Files.**
- `Cargo.toml` (workspace members): add `crates/flare-device`.
- `crates/flare-device/Cargo.toml`, `src/lib.rs`: new crate, edition 2024, deps `serde`, `serde_json`,
  `flare-surfaces` (for cursor timing constants).
- `src/target.rs`: `Target::Sim { udid }`, parse / name `sim/<udid>` (36-char UUID, lower-cased), `clean_target`
  limits as `flare-surfaces/src/state.rs:44`.
- `src/source.rs`: `trait DeviceSource`, `Touch {phase, x, y, second: Option<(f64,f64)>}`, `Key`, `Button`,
  `SourceEvent`, `DeviceState`, `DeviceInfo {target, name, kind, os, state, booted_by_flare}`.
- `src/fake.rs`: `FakeSource`: scripted devices, a frame counter that "renders" a solid colour per seq, records every
  input, can be told to fail, go broken, or be slow to boot.
- `src/cap.rs`: `FrameCap` (15 ms spacing, newest wins, a lone frame goes at once), `Ring` slot ownership
  (`take_free`, `shown`, `done`; `RING_SLOTS = 3`).
- `src/geom.rs`: panel point to device point (letterbox), device point to portrait ratio for the four
  orientations, swipe sampling every 8 pt.
- `src/keymap.rs`: US keymap char to HID usage plus shift (port of idb `Keymap.swift`, MIT notice).
- `src/tree.rs`: `UiNode`, `filter_tree`, refs `d1..`, cap 300, cleaned labels.
- `src/indicator.rs`: holds a `flare_surfaces::agent_cursor::AgentCursor` (the dot: glide, rest memory, ripples,
  fade, animations off) and an `AgentBorder` (the glow) with device points as the viewport, and adds only the trail
  (last 250 ms of points) and the keyboard badge, on an injected clock.
- `src/job.rs`: `DeviceOp`, `Job` (glide hold, send, wait for the next "changed" count or 300 ms, answer),
  `Fail::{Interrupted, Unavailable, NotFound, Busy, Retry, Forbidden}`, shaped like `flare-surfaces/src/agent_job.rs`.
- `src/wire.rs`: the sim host protocol: JSON control lines (`hello`, `list`, `attach`, `detach`, `pause`, `resume`,
  `touch`, `key`, `button`, `text`, `snapshot`, `ui_tree`, `launch`, `install`, `boot`, `shutdown`, `quit`), the
  mach records (frame `(device, slot, seq, t_us)`, slot done `(device, slot)`, changed `(device, count)`) as plain
  structs with their byte layout, `PROTOCOL = 1`.
- `src/versions.rs`: tested table, floor (Xcode 26), `input_path(coresim_version) -> Dtuhid | Indigo`.
- `crates/flare-surfaces/src/kind.rs`: `PanelKind::Device`, name `"device"`, `is_native` true, **`is_built` false**
  (kept in the file, not shown: a build of PR 1 to 3 must not hand `sim/<udid>` to the file preview). PR 4a flips it.
  Tests extended.
- `crates/flare-surfaces/src/store.rs`: round-trip test for a device panel.
- Docs: `docs/regression-rules.md` (1316 first half), `docs/surfaces/decisions.md` (new crate, Rust sim host,
  point coordinates).

**Tests first.**
- `flare_device::target::a_sim_target_round_trips_and_bad_udids_are_refused`
- `flare_device::cap::seventy_seven_callbacks_a_second_become_sixty`
- `flare_device::cap::the_newest_held_frame_wins`
- `flare_device::cap::a_lone_frame_is_sent_at_once`
- `flare_device::cap::a_slot_comes_back_only_after_its_work_is_done`
- `flare_device::cap::a_slot_flare_holds_is_never_handed_out`
- `flare_device::cap::with_no_free_slot_the_frame_is_dropped_and_counted`
- `flare_device::geom::panel_points_map_to_device_points_through_the_letterbox`
- `flare_device::geom::points_map_to_portrait_ratios_in_every_orientation`
- `flare_device::geom::a_swipe_is_sampled_every_eight_points`
- `flare_device::keymap::ascii_types_with_shift_where_needed_and_other_text_needs_paste`
- `flare_device::tree::zero_size_and_unlabelled_full_screen_groups_are_dropped`
- `flare_device::tree::refs_are_numbered_in_reading_order_and_capped_at_300`
- `flare_device::tree::labels_are_cleaned`
- `flare_device::indicator::the_dot_is_the_agent_cursor_in_device_points`
- `flare_device::indicator::the_trail_keeps_only_the_last_250_ms`
- `flare_device::indicator::the_badge_shows_while_typing_and_names_the_key`
- `flare_device::indicator::hide_clears_dot_trail_badge_and_glow_at_once`
- `flare_device::job::a_tap_waits_for_the_glide_then_for_the_next_change`
- `flare_device::job::with_no_change_the_job_answers_after_300_ms`
- `flare_device::job::a_takeover_ends_the_job_interrupted`
- `flare_device::job::a_cancel_ends_the_job_at_once`
- `flare_device::job::a_stale_ref_is_not_found`
- `flare_device::wire::every_message_round_trips`
- `flare_device::versions::old_coresimulator_uses_indigo_and_new_uses_dtuhid`
- `flare_surfaces::kind::device_round_trips_by_name`
- `flare_surfaces::kind::device_is_not_built_yet`
- `flare_surfaces::store::a_device_panel_round_trips_in_panels_json`

Keep green: `flare_surfaces::kind` and `store` tests, rule 240 tests.

**Done when.** All gates pass on Mac and Windows CI; no behaviour change in the app.

---

## PR 2: `flare-sim-host`, frames

Branch `feat/device-panel-2`. Ships alone (built, not yet started by Flare). Needs S2 and S3 answered.

**Files.**
- `Cargo.toml`: workspace deps `objc2-metal =0.3.2`, `objc2-io-surface =0.3.2`; README "Dependencies" bullets;
  `THIRD-PARTY-NOTICES.md`.
- `crates/flare-device/Cargo.toml`: `[[bin]] flare-sim-host` (path `src/bin/flare_sim_host.rs`), macOS-only deps
  objc2, block2, dispatch2, libloading, objc2-foundation, objc2-metal, objc2-io-surface.
- `src/bin/flare_sim_host.rs`: argv (`--rendezvous <name>`, `--bench <udid> <s>`, `--probe-barcode`, `--self-test`),
  stdio loop, logs to `<profile>/logs/sim-host.log` (path in argv); the Windows stub.
- `src/macos/load.rs`: Xcode from `DEVELOPER_DIR` (Flare sets it; `xcode-select -p` when run by hand), dlopen
  CoreSimulator from `/Library/Developer/PrivateFrameworks` and SimulatorKit from that Xcode (SharedFrameworks first,
  then the Xcode 26 path), read both `CFBundleVersion`s, `hello`.
- `src/xcode.rs` (pure plus a worker call): pick the Xcode (spec "Setup" state 2 order), Command Line Tools count as
  none. Used by Flare's side in PR 4b; tested here.
- `src/macos/guarded.rs`: `guarded(sel, || ...)`: `respondsToSelector:` check, `objc2::exception::catch`, 5 s
  timeout for callback calls, `Broken` with the symbol name.
- `src/macos/sim.rs`: service context, device set, device by UDID, device type (`mainScreenScale`,
  `mainScreenSize`), `io` / `ioPorts` / descriptor, the `SimScreenAdapter` LCD screen, `framebufferSurface`, register
  and unregister callbacks (plain message sends, no KVC), the "changed" counter. Fallback to
  `registerCallbackWithUUID:ioSurfacesChangeCallback:` as idb.
- `src/macos/ring.rs`: `RING_SLOTS` RGBA IOSurfaces, MTLTextures, the swizzle render pass (A = 255), slot states,
  frees an old ring after all its slots are done.
- `src/macos/rendezvous.rs`: both sides: `check_in(name)` (Flare) and `connect(name)` (sim host); ports, frame,
  slot-done and changed records over mach; audit-token pid check on both sides.
- `src/macos/simctl.rs`: `list`, `boot` + `bootstatus -b`, `shutdown`, `screenshot` (still mode), `launch`,
  `install`, `pbcopy`, `create` / `delete` (tests and bench only), each on a worker with a timeout, with
  `DEVELOPER_DIR` set.
- `src/macos/bench.rs`: port of `zc.m` reading the probe app's barcode: distinct, skipped, interval p50 / p99, GPU
  ms, "no free slot" drops, process CPU (`getrusage`), JSON out. Makes its own device and deletes it.
- `src/client.rs` (`SimSource`): spawn with `flare_proc::guard::guard_command` (stdin, stdout, stderr only; no
  `pre_exec` of its own), the mach reader thread, slot-done sender, restart policy (3 in 5 minutes, then broken),
  still-mode loop.
- `crates/flare-win/src/names.rs`: `SIM_HOST_EXE = "flare-sim-host"`.
- `tools/device-probe/`: the probe app (`FPSProbe-App.swift`) moved in from research, plus a `build.sh` that builds
  it for the simulator with `xcrun swiftc -target arm64-apple-ios<ver>-simulator` and a checked-in `Info.plist`
  (no xcodegen, which neither the runner nor a fresh Mac has). Tests only, never shipped, never opens Xcode.
- Docs: rules 1310, 1311 (sim host half), 1312 (sim host half); `docs/surfaces/decisions.md`; `docs/features.md`
  entry "Device panel, PR 2: the sim host (not yet shown)".

**Tests first.**
- `flare_device::client::the_sim_host_is_restarted_three_times_then_broken` (fake binary via `use_test_entry`)
- `flare_device::client::a_broken_sim_host_falls_back_to_still_pictures_at_three_fps_at_most`
- `flare_device::client::frame_records_keep_only_the_newest_per_device`
- `flare_device::client::a_protocol_mismatch_in_hello_is_refused`
- `flare_device::client::the_sim_host_gets_developer_dir_and_no_extra_descriptors`
- `flare_device::xcode::command_line_tools_alone_are_no_xcode`
- `flare_device::xcode::the_newest_xcode_app_is_picked_when_xcode_select_points_elsewhere`
- `flare_device::macos::guarded::a_missing_selector_is_broken_with_its_name_not_a_crash`
- `flare_device::macos::guarded::an_objc_exception_is_caught`
- `flare_device::macos::rendezvous::ports_from_another_pid_are_refused`
- `flare_device::macos::ring::an_old_ring_is_freed_only_after_every_slot_is_done`
- `flare_device::macos::load::self_test_finds_every_class_and_selector` (CI macOS runner; skipped with a reason if no
  Xcode)

Keep green: `flare-proc` guard tests (rules 1053, 1054), `flare-mods` helper tests.

**Before merge.** On the Mac mini, headless: `flare-sim-host --bench new 15` (it makes and deletes its own device)
against the probe app (light and `-heavy`): at least 891/900 distinct, interval p99 under 25 ms, zero "no free slot"
drops with a fake consumer that frees each slot 17 ms late, CPU under 10% of a core. Paste the JSON in the PR.

---

## PR 6: Bundle and sign the sim host (merges right after PR 2)

Branch `feat/device-panel-6`. Ships alone after PR 2: the bundle carries `flare-sim-host`, nothing starts it yet.
Needs S3 answered.

**Files.**
- `tools/release/mac/make-app.sh` (~47-53): `cargo build --release --locked -p flare-device --bin flare-sim-host`.
- `crates/flare-cef/src/macos/release.rs` (~411-440): `need(i.bin_dir.join(SIM_HOST_EXE))`, add to
  `executables` beside `mcp` and `host` (signed `Role::Tool` by `bundle.rs`, identifier
  `com.flare.terminal.flare-sim-host`, hardened runtime, no entitlements).
- `tools/release/mac/pack.sh` (~89): add `flare-sim-host` to the `for t in UpdateMac flare-mcp flare-parakeet-host`
  loop (identifier, runtime, no entitlements). If S3 showed an entitlement is needed, this check, `sign.rs` and
  `entitlements(Role::Tool)` gain a sim-host case instead, with a test, and the spec records it.
- `crates/flare-cef/src/macos/sign.rs`: no change expected (Tool role).
- `.github/workflows/macos-app.yml`: nothing new beyond `make-app.sh`; a step runs the built bundle's
  `Contents/MacOS/flare-sim-host --self-test` (skips with a reason if the runner's Xcode is under the floor).
- `.github/workflows/ci.yml` (`macos` job): `cargo run -p flare-device --bin flare-sim-host -- --self-test`, same
  skip.
- Docs: `docs/releasing-macos.md` (bundle layout lists `MacOS/flare-sim-host`), `release.rs` doc comment,
  `docs/features.md` line.

**Tests first.**
- `flare_cef::release::the_bundle_lists_the_sim_host_as_a_tool` (in `crates/flare-cef/tests/mac_release_files.rs` if
  the bundle list is tested there, else beside the existing `release.rs` tests)
- `flare_cef::sign::the_sim_host_is_signed_as_a_tool_with_no_entitlements`

**Before merge.** Build a notarized test DMG on CI. On the Mac mini, from the staged bundle (not over the running app,
and without starting Flare): `codesign --verify --deep --strict`, `flare-mac-app verify`, then
`Contents/MacOS/flare-sim-host --self-test` and `--bench new 15` (headless) pass. This is S3 on the real release
path; the by-hand checks wait for PR 4b.

---

## PR 3: `flare-sim-host`, input and UI tree

Branch `feat/device-panel-3`. Ships alone (unwired). Built per open decision 2 (S1 result).

**Files.**
- `src/macos/xpc.rs`: the libxpc externs (`xpc_endpoint_create_mach_port_4sim` and
  `xpc_connection_enable_sim2host_4sim` via dlsym), connection lifecycle, liveness barrier (4 s, five tries), 200 ms
  settle, reconnect once on interruption.
- `src/macos/dtuhid.rs`: `IndigoDigitizerEvent`, `IndigoKeyboardButtonEvent`, `IndigoButtonEvent` dictionaries
  (uint64 integers), send plus barrier, the contact tracker.
- `src/macos/indigo.rs`: legacy `SimDeviceLegacyHIDClient` path for older CoreSimulator (single-touch rebuild as
  idb), never mixed with DTUHID.
- `src/macos/bench.rs`: `--input-bench <udid|new>` (port of the S1 probe: taps, swipes, characters, latency from
  the probe app's painted touch frame), JSON out.
- `src/macos/ax.rs`: `AXPTranslator` with a bridge-token delegate, `sendAccessibilityRequestAsync`, one request in
  flight, tree to `UiNode`.
- `src/macos/snapshot.rs`: `framebufferSurface` itself (not a ring slot, so it is fresh while the stream is paused)
  scaled to point size by a Metal pass, read back and encoded as PNG with ImageIO, on the sim host's worker.
- `src/macos/orient.rs` (S5 found a way, decision 11): set the orientation with a Purple GSEvent 50 to
  `PurpleWorkspacePort`, read it from CoreDevice `displayinfo`; the `rotate` control line; the orientation in the
  `Frames` event and an event when it changes.
- No AXe fallback: S1 passed, so open decision 2 is a (no `axe.rs`).
- Docs: rules 1313, 1314, 1315 (sim host half); research link; `docs/surfaces/decisions.md` (input path chosen,
  with S1 numbers).

**Tests first.**
- `flare_device::macos::dtuhid::a_tap_is_start_then_end_with_portrait_ratios`
- `flare_device::macos::dtuhid::integers_are_sent_as_uint64_and_state_zero_is_never_sent`
- `flare_device::macos::dtuhid::buttons_use_consumer_page_usages`
- `flare_device::macos::xpc::no_liveness_reply_after_five_tries_is_broken_input`
- `flare_device::macos::indigo::indigo_is_never_used_after_dtuhid_in_one_boot`
- `flare_device::macos::ax::one_request_is_in_flight_at_a_time`
- `flare_device::client::touches_wait_up_to_twenty_seconds_for_the_connection_then_drop`
- `flare_device::client::non_keymap_text_goes_through_the_simulator_pasteboard_not_the_macs`
- `flare_device::macos::snapshot::a_snapshot_while_paused_is_the_current_screen` (macOS, real simulator, skipped
  with a reason without one)

**Before merge.** On the Mac mini, headless, on a device made for the run: a hand-run `--input-bench new` (100 taps,
20 swipes, 100 chars, with the probe app): tap p50 at most 50 ms, 100/100 landed, 100/100 characters. Paste the
numbers.

---

## PR 4a: The panel for the person, on the fake source

Branch `feat/device-panel-4a`. Ships alone: the device kind is offered only under `--e2e` (fake source) and in
tests, so a release with it shows nothing new. Mockup step first.

**Step 0, before code: mockup** (impeccable; `docs/superpowers/specs/2026-11-19-device-panel-mockup.html`): the
header (device name, Home, Lock, Rotate, menu), each setup state, the still badge and line, the "slow" mark, the
Settings row. Claude gives Max the file path and does not open it (Zen is not installed on the Mac; no other browser).
Max picks; the spec gets an "Update, Max, <date> (after the mockups)" section.

**Files.**
- `crates/flare/Cargo.toml`: dep `flare-device`.
- `crates/flare-surfaces/src/kind.rs`: `is_built` true for `Device`.
- `crates/flare/src/device_ctl.rs` (new): `DeviceCtl` per tab (state machine of the setup states, the source behind
  `dyn DeviceSource`, generation numbers), `sync` (sets `panel-kind` 5, the `panel-device-*` props), `page_pointer` /
  `page_wheel` / `page_key` with `person_input`, the CPU path (a source frame as `SharedPixelBuffer`), pause on close
  / behind / minimised / occluded, view-only for a second tab on the same device.
- `crates/flare/src/appctl/device_glue.rs` (new): open and close, `UiMsg::Device` handling.
- `crates/flare/src/appctl.rs`: `sync_panels` (~3792) calls `device.sync` after `web.sync`; the Slint callbacks bound
  next to `on_panel_web_pointer` (~1653); `UiMsg::Device` routing.
- `crates/flare/src/panel_ctl.rs`: `target_ok` (~361) gets a `PanelKind::Device` arm that checks only the target's
  shape (`flare_device::target::parse`); `snap` device branch (~229, ~879); `sync` (~970) skips the device kind as it
  skips web; `persist` / `restore` keep device panels; restore never boots.
- `crates/flare/ui/panel.slint`: kind 5 body: `device-box` with the `Image` (`panel-device-frame`), letterbox,
  rounded corners, TouchArea, `device-keys` FocusScope, the setup states, still badge, slow mark; header buttons.
- `crates/flare/ui/app.slint`: `panel-device-*` props (~505-525 pattern), callbacks.
- `crates/flare/src/ui_tests/device_panel.rs` (new) and its `mod` line in `ui_tests/mod.rs`.
- `crates/flare/src/appctl/tests/device_input_tests.rs` (new), shaped like `page_keys_tests.rs`.
- `tools/e2e/src/scenarios/device.rs`: `devicePanel` with `FakeSource` under `--e2e`; test hook `DeviceUserInput`;
  probe `device.state`. Runs on the Windows and the Mac e2e jobs (not in `mac_gate.rs`).
- Docs: rules 1316, 1318, 1320 (person half); `docs/surfaces/lessons.md` by-hand list; `docs/e2e.md` the new
  scenario, hook and probe; `docs/parity.md` one DIFF row if the checklist needs it (a panel kind Prism did not have).

**Tests first.**
- `ui_tests::device_panel::kind_five_lays_out_like_the_web_panel_at_every_scale` (100/150/225/300%, both looks)
- `ui_tests::device_panel::the_picture_is_letterboxed_to_the_device_aspect`
- `ui_tests::device_panel::each_setup_state_shows_one_line_and_at_most_one_button`
- `ui_tests::device_panel::no_xcode_offers_get_xcode`
- `ui_tests::device_panel::still_mode_shows_the_badge_and_the_reason`
- `ui_tests::device_panel::the_cpu_path_shows_the_slow_mark`
- `ui_tests::device_panel::a_second_tab_on_the_same_device_is_view_only`
- `appctl::tests::device_input_tests::a_click_becomes_touch_start_and_end_in_device_points`
- `appctl::tests::device_input_tests::a_wheel_becomes_a_short_drag_at_the_pointer`
- `appctl::tests::device_input_tests::points_outside_the_screen_are_dropped`
- `appctl::tests::device_input_tests::holding_the_splitter_is_not_a_takeover`
- `appctl::tests::device_input_tests::the_stream_pauses_when_the_tab_goes_behind_and_resumes_with_the_current_frame`
- `flare::panel_ctl::device_panels_are_persisted_and_restored`
- `flare::panel_ctl::a_device_target_is_checked_by_shape_not_as_a_file`
- `flare::panel_ctl::restore_shows_boot_and_never_boots`
- `flare::panel_ctl::a_deleted_device_drops_the_panel_once_the_list_is_read`
- e2e: `devicePanel` (Windows and Mac jobs)

Keep green: rule 240 tests, web panel layout tests, `page_keys_tests`, `panelSplit`, render notifier tests.

**Done when.** All gates pass; `devicePanel` passes on both e2e jobs; a Mac release build with no `--e2e` shows no
Device entry.

---

## PR 4b: The real simulator, zero copy

Branch `feat/device-panel-4b`. The first visible change. Needs PR 2, 3 and 6 merged.

**Files.**
- `crates/flare/src/device_frames.rs` (new, macOS): `DeviceFrames` (ring textures via
  `wgpu::hal::metal::Device::texture_from_raw` + `create_texture_from_hal`, `Rgba8Unorm`,
  `TEXTURE_BINDING | RENDER_ATTACHMENT`, made on wgpu's own `MTLDevice`; newest slot; slots in use), `frame_image`
  for `BeforeRendering`, `after_render` that registers `on_submitted_work_done` for the slot shown before, the CPU
  fallback (`IOSurfaceLock` read-only into `SharedPixelBuffer`, at most 30 fps) when the import is refused.
- `crates/flare/src/window.rs`: a weak `DeviceFrames` slot beside `slide_ctl`, set at startup;
  `BeforeRendering` (~938) calls `frame_image` after the grid; `AfterRendering` (~969) calls `after_render`;
  `RenderingSetup` (~920) rebuilds ring textures; `RenderingTeardown` (~986) drops them. `try_borrow_mut` only.
- `crates/flare/src/device_ctl.rs`: `SimSource` on a Mac with Xcode (the Xcode pick from `flare_device::xcode` on a
  worker), the frame reader's `UiMsg::Device` with the `WakeGate` pattern, the panel menu entry (macOS, Apple
  silicon), quit shutdown of booted-by-Flare devices (one detached `simctl shutdown`, not waited on).
- `crates/flare-settings`: `flare.device.shutdown_on_quit` row in a new section `device` ("Device panel") at the end
  of the Agents page (`sections.rs`, `pages.rs`), macOS only, find keywords simulator, iPhone, device.
- `crates/flare-diag`: include `sim-host.log` and the hello facts.
- `.github/workflows/ci.yml` (`e2e-macos`, ~156): build `-p flare-device` too, so `deviceReal` finds the sim host.
- `tools/e2e/src/scenarios/device_real.rs`: `deviceReal` (opt-in, full-name filter only, GitHub macOS runner; skips
  with a reason when open check 6 found no Xcode or runtime there).
- Docs: rules 1311 (Flare half), 1312 (Flare half), 1317, 1319; `docs/features.md` "Device panel (issue #248),
  PR 4b: see and use a simulator"; `docs/e2e.md` `deviceReal`.

**Tests first.**
- `flare::device_frames::an_iosurface_imports_as_a_slint_image_and_reads_back` (macOS; its own wgpu device, no window)
- `flare::device_frames::a_slot_is_freed_only_from_the_work_done_callback`
- `flare::device_frames::a_refused_import_falls_back_to_the_cpu_path`
- `flare::device_ctl::quit_shuts_down_only_devices_flare_booted_and_does_not_wait`
- `flare::device_ctl::the_menu_entry_is_offered_only_on_apple_silicon_with_xcode`
- `flare::settings::find_device_shutdown_row`
- e2e: `devicePanel` (still green), `deviceReal` (opt-in)

Keep green: render notifier tests, the grid's GPU path, `sidebarSlide` frame tests.

**Before merge.**
- `deviceReal` on the GitHub runner: `gh workflow run ci --ref feat/device-panel-4b -f e2e_only=true -f
  e2e_filter='deviceReal'` (the `e2e-macos` job runs on a hand run; `e2e_only` skips only `check` and `macos`).
- On the Mac mini, headless: a staged build's `FLARE_TRACE_FRAME=1` log while the probe app runs `-heavy` in a panel
  shows the UI thread under 4 ms p99 per frame and Flare's CPU noted in the PR. Getting this log needs the staged
  Flare on screen, so Claude asks Max first (AskUserQuestion) or Max runs it.
- Max's by-hand checks 1, 2, 3, 6, 7 from the spec on the staged build (not over his running `Flare.app`).

---

## PR 5: Agent tools, indicators and takeover

Branch `feat/device-panel-5`. Ships alone, after PR 4b. If it grows past one review, split it at the line between
the ctl and MCP half (method, grant, ctl_app, flare-mcp) and the indicator half (device_ctl, panel.slint).

**Files.**
- `crates/flare-ctl/src/method.rs`: `Namespace::Device` (prefix `device`), 12 `Method::Device*` variants, `ALL`
  count, `name()`, `namespace()`, `timeout()` (`DEVICE_BOOT_TIMEOUT` 130 s for boot, `BROWSER_TIMEOUT` 60 s for the
  rest), `permission()` none.
- `crates/flare-ctl/src/grant.rs`: `Grant::Agent` `N::Device => true` (~215); `Namespace::ALL` 21 to 22; `Remote`,
  the host bridge grant and `Ui` stay closed to it; tests that iterate every namespace and method;
  `permission_names()` unchanged.
- `crates/flare/src/ctl_app.rs`: `CtlOp::DeviceAgent {tab, op, cancel}` and its Debug arm; `device_op(method,
  params)` validator (`bad_params`); `call_inner` arm with `tab_of_call` and `ask_until`; `capabilities()` (~2911)
  gated on macOS, Apple silicon, Xcode found and a Flare that shows a window (or `--e2e` with the fake source);
  `unavailable` for a tab whose session runs on another machine or is run hidden for one; `panel.open` refuses
  `kind: device` naming `device_boot`; `device_install` resolves symlinks before the folder check.
- `crates/flare/src/appctl.rs` (~4471, ~5697): route `DeviceAgent` to `ctl_device_agent`; error if it reaches the
  sync path.
- `crates/flare/src/appctl/device_glue.rs`: `ctl_device_agent` (cancel, shell tab, the caller's own panel, open or
  boot for `device_boot`, `start_agent`).
- `crates/flare/src/device_ctl.rs`: `start_agent`, `finish_agent`, `agent_tick`, `user_input` (ends with
  `interrupted`, hides indicators); agent tag via `cursor_tag` (`web_ctl.rs:366`, app crate); the edge glow and hold.
- `crates/flare/ui/panel.slint`: indicator layer (dot, ripple, trail, badge) over `device-box`, glow.
- `crates/flare-mcp/src/device.rs` (new): `tools()`, `call()`, `device_error()`, `render_device_look` (text then PNG,
  markers).
- `crates/flare-mcp/src/lib.rs`: `mod device;`; `tools/list` (~919) adds `device::tools()` only when the channel's
  `hello` capabilities have `device.look` (a `device_tools(channel)` beside `mod_tools(channel)`, cached per
  connection); the `call_tool` chain (a device tool called when not listed answers `unavailable`); server
  instructions line, only when the tools are listed; tool-order test.
- `crates/flare-mcp/src/hook.rs`: optional `PanelPage::Device` so the prompt hook says a device is open.
- `crates/flare-mcp/tests/stdio.rs`: the no-Flare count stays 24 (`:117`) and the mod index stays 24 (`:309-313`);
  a new in-process test with a fake channel whose `hello` lists `device.look` counts 36.
- `crates/flare-mods/src/tools.rs`: built-in names test gets the 12 `device_*` names.
- `crates/flare-agent/claude-plugin/commands/device.md` (optional slash command, allowed-tools frontmatter) and the
  plugin.rs test.
- `tools/e2e/src/scenarios/device.rs`: `deviceAgent`, `mcpDevice` (FakeSource; takeover through `DeviceUserInput`).
- Docs: rules 1315 (tool half), 1320 (agent half), 1321, 1322; `docs/surfaces/protocol.md` (`device.*` methods,
  grants table section 4, MCP section 9); `docs/features.md` "Device panel, PR 5: the agent drives it" with the new
  tool count; `docs/e2e.md`; README Features line.

**Tests first.**
- `flare_ctl::method::device_methods_are_named_under_device`
- `flare_ctl::grant::only_the_agent_grant_enters_device`
- `flare_ctl::grant::no_permission_opens_device_for_a_mod`
- `flare::ctl_app::device_calls_act_on_the_callers_own_tab_only`
- `flare::ctl_app::device_shutdown_is_forbidden_for_a_device_flare_did_not_boot`
- `flare::ctl_app::device_install_outside_the_tab_folder_or_derived_data_is_forbidden`
- `flare::ctl_app::panel_open_refuses_kind_device_and_names_device_boot`
- `flare::ctl_app::capabilities_list_device_only_on_a_mac_with_xcode`
- `flare::ctl_app::a_remote_or_hidden_session_gets_unavailable_and_nothing_boots`
- `flare::ctl_app::device_install_follows_symlinks_before_the_folder_check`
- `flare_ctl::grant::remote_and_host_grants_never_enter_device`
- `flare_mcp::device::the_tools_are_listed_with_descriptions_written_for_an_agent`
- `flare_mcp::device::schemas_refuse_unknown_keys`
- `flare_mcp::device::look_returns_text_first_then_a_png`
- `flare_mcp::device::ui_text_is_fenced_in_page_markers`
- `flare_mcp::device::interrupted_says_the_user_took_over`
- `flare_mcp::lib::tool_order_includes_device_tools_after_browser_tools`
- `flare_mcp::lib::device_tools_are_listed_only_when_flare_has_them`
- `ui_tests::device_agent::the_finger_dot_glides_to_the_tap_and_ripples`
- `ui_tests::device_agent::a_swipe_leaves_a_fading_trail`
- `ui_tests::device_agent::typing_shows_the_keyboard_badge`
- `ui_tests::device_agent::the_persons_press_ends_the_agent_call_interrupted_and_hides_the_dot`
- `ui_tests::device_agent::indicators_are_never_in_the_look_picture`
- `ui_tests::device_agent::a_cancel_ends_the_job_and_hides_the_glow`
- `ui_tests::device_agent::still_mode_answers_input_tools_unavailable_with_the_reason`
- e2e: `deviceAgent`, `mcpDevice`

Keep green: `agentBrowser`, `mcpBrowser`, rule 229 and 254 tests, every grant iteration test.

**Before merge.** Max's by-hand checks 4 and 5 from the spec, on a staged build on the Mac mini (not over his
running `Flare.app`), with Claude in a tab.

---

## Docs

- `docs/regression-rules.md`: rules 1310 to 1322 in `## Device panel (2026-11-19)`.
- `docs/surfaces/protocol.md`: `device.*`, grants table, MCP tools, `panel.state` device kind.
- `docs/surfaces/decisions.md`: sim host in Rust, ring handover, coordinates, no AXe (or AXe v2.0.4 fallback),
  restore never boots.
- `docs/surfaces/lessons.md`: by-hand list for the device panel.
- `docs/features.md`: one entry per PR (PR 2, 6, 4b, 5), tool count 36 on a Mac with Xcode, 24 elsewhere.
- `docs/e2e.md`: `devicePanel`, `deviceAgent`, `mcpDevice`, `deviceReal` (opt-in), the `DeviceUserInput` hook, the
  `device.state` probe.
- `docs/releasing-macos.md`: the new tool binary.
- README: Features line (PR 5), Dependencies bullets (PR 2).
- Research: `research/flare/2026-11-19-ios-simulator-input.md` (step 0, with S1 to S6), linked from the spec and
  listed in `research/README.md` under the flare topic.

## Follow-ups (not in these PRs)

- Android emulator as a second `DeviceSource` (Mac and Windows).
- H.264 (VideoToolbox, low-latency) for showing a device in a remote tab.
- A display-link pacer for 120 Hz displays.
- A downloadable sim host that updates between Flare releases (if open decision 4 goes that way later).
- Real iPhones over USB.
- Plugins' access to the device panel (a `device` permission), if Max wants it.

## Open checks for the builder (verify before coding, note the answer in the PR)

### Step 0 results (2026-11-19)

Full numbers: `~/Documents/Claude/research/flare/2026-11-19-ios-simulator-input.md`. Mac mini (M6), macOS 27.2,
Xcode 27.2 (27B52), CoreSimulator 1174.21, iPhone 18 Pro on iOS 27.2, every spike on its own simulator, all headless.
Every gate passed.

| Spike | Result |
|---|---|
| S1 input (gate) | Pass, 3 runs. Tap p50 15.1 to 17.5 ms (all 300: p50 15.8, p99 58.6, max 162.8); 300/300 landed within 0.17 pt; 300/300 characters; swipes 60/60 with every move; pinch (`pointTwo`) works. Cold connect after a fresh boot: 2.0 s, 11.6 s and 8.0 s (2 of 3 needed a second try); warm 6 to 32 ms. First key of a session 200 to 690 ms. |
| S2 ring handover | Pass. 0 wrong bytes in all 3 slots, `Image::try_from` accepts them, forged frame record and ring refused 2/2, 600 records each way, round trip p99 6.8 to 41.5 us. Three corrections (below). |
| S3 signing | Pass. Developer ID and ad hoc, hardened runtime, no entitlements: frames, DTUHID tap, AX dlopen, `simctl launch` and `screenshot` all work from a signed parent; no TCC request. Notarized path untested (PR 6). |
| S4 UI tree | Settings 16 elements, 15 to 39 ms warm (66 XPC calls); 3.3 s on the first request after a boot; no prompt. `AXIsProcessTrusted` triggers a TCC preflight: never call it. |
| S5 rotation | A way: set with a Purple GSEvent 50 to `PurpleWorkspacePort`, read `currentOrientation` from CoreDevice `displayinfo` (1 to 4 ms). The framebuffer stays portrait 1206x2622 (same surface, no surfaces-changed callback). |
| S6 colour | Display P3 (colour space 7, P3_D65, sRGB transfer, no ICC profile). |

Corrections the PRs take from the spikes:
- PR 2 (`rendezvous.rs`): the sim host's first message carries a **send** right to its own receive port; the audit
  pid is checked on **every** message on both sides, and a refused message is freed with `mach_msg_destroy`
  (test `ports_from_another_pid_are_refused` covers a forged ring and a forged frame record).
- PR 2 (`ring.rs`) and PR 4b (`device_frames.rs`): the IOSurface `MTLTexture` uses storage mode Shared and usage
  `ShaderRead | RenderTarget`; the wgpu descriptor adds `COPY_SRC` (needed by the read-back test).
- PR 2 (`ring.rs`): the swizzle pass converts Display P3 to sRGB (a 3x3 matrix in linear light), so colours match
  Simulator.app (by-hand check 2).
- PR 3: touches wait up to **20 s** for the connection (not 5 s), the same as the agent's wait; the test becomes
  `touches_wait_up_to_twenty_seconds_for_the_connection_then_drop`. The agent's typing wait allows 1 s for the first
  key of a session. The sim host sends one AX request right after attach to warm the tree. Never `AXIsProcessTrusted`.
- PR 3 (`orient.rs`, in scope): Purple GSEvent 50 to set, `displayinfo` to read (polled after a rotate, or on
  `propertiesChangedCallback`). Before merge, `--input-bench` taps in landscape to settle whether touch ratios are
  un-rotated (idb: the digitizer stays portrait); the result goes in `geom.rs`.
- PR 4b: the ring does not change on rotation; Flare rotates the picture by the orientation in the `Frames` /
  orientation event (rot90 and rot270).
- PR 6: no entitlement, no `pack.sh` or `sign.rs` sim-host case.

### Decisions taken (delegated by Max, 2026-11-19: the spec's recommendation, adjusted by Step 0)

1. Own private-API code for frames: yes, in `flare-sim-host` only.
2. Input path: **a**, own DTUHID code (legacy Indigo below CoreSimulator 1155.4), no AXe. S1 passed the gate.
3. Sim host in Rust with objc2. **3b**: not needed, S2 passed; no in-process frame path is built. A rendezvous that
   fails at run time goes to still mode, as the failure-mode table says.
4. The sim host ships inside `Flare.app`.
5. Restore shows the last device with Boot; never auto-boots.
6. Shut down at quit the simulators Flare booted: default on.
7. Plugins: no device access in v1.
8. Xcode 26: best effort via legacy Indigo, untested by us.
9. 120 Hz displays: newest frame wins, no display link in v1.
10. Mockup step before PR 4a: yes.
11. Rotation: **in v1** (S5 found a headless way). The Rotate buttons and `device_press` `rotate_left` /
    `rotate_right` ship; the picture is rotated in Flare.
12. `device_*` tools are listed only where they work.

Also from S1: the Alt-drag two-finger pinch ships in v1.

### Checks

1. S1: DTUHID tap latency and landing on Xcode 27.2 (gate). Answered 2026-11-19: pass (see above).
2. S2: mach-port ring handover and wgpu import from another process. Answered 2026-11-19: pass.
3. S3: a Developer ID hardened-runtime tool with no entitlements can dlopen SimulatorKit and CoreSimulator.
   Answered 2026-11-19: yes, and ad hoc too; notarized build still to prove in PR 6.
4. S4: AX tree time on a 150-element screen. Answered 2026-11-19: no 150-element screen found; Settings 16 elements
   in 15 to 39 ms, about 1 to 2.4 ms per element, 3.3 s once per boot.
5. Whether `xcrun simctl` needs `xcodebuild -runFirstLaunch` on a fresh Xcode, and which error text says so (for
   setup state 3).
6. Whether the GitHub macOS runner image has an iOS runtime and Xcode 27 (for `deviceReal` and `--self-test`).
7. Whether `IOSurfaceLock` on the RGBA ring is fast enough for the CPU fallback at 30 fps (about 12 MB a frame).
8. Rule numbers 1310 to 1322 still free on origin/main.
9. S5: rotation, headless set and read, and whether the framebuffer changes size. Answered 2026-11-19: Purple
   GSEvent 50 sets it, `displayinfo` reads it, the framebuffer stays portrait. Touch un-rotation answered 2026-11-19
   (PR 3 `--input-bench`): landscape points are turned back to portrait (`geom::to_portrait_ratio`), 20 of 20 taps in
   rot90 and rot270 landed within 0.17 pt.
10. S6: the framebuffer's colour space. Answered 2026-11-19: Display P3.
11. Whether `bootstrap_check_in` of an undeclared per-pid name works from a hardened-runtime, Developer ID signed
    Flare (Chromium's rendezvous does it; S2 and S3 together answer it). Answered 2026-11-19: yes (kr 0, Developer ID
    and ad hoc parent).
12. Whether `RING_SLOTS` 3 is enough with the one-frame-late release (PR 2 bench with a late consumer, PR 4b trace).
13. Whether a Settings row can be macOS only today, or the row shows on Windows greyed with a note. Answered 2026-11-20 (PR 4b): yes,
    with `cfg!(target_os = "macos")` in the Agents page's builder and in Find's `drawn` (`device_section_drawn`).

PR 4b answers (2026-11-20, Mac mini, Xcode 27.2):
- 5: not reproducible here (this Xcode is set up). The list's error is read as "not set up" when it names
  `runFirstLaunch`, "first launch" or the licence (`device_ctl::not_set_up`); anything else is the Failed state.
- 6: `deviceReal` checks it on the runner (an Xcode selected, an iOS runtime, else it skips with the reason); not run
  yet from this branch.
- 7: yes. `IOSurfaceLock` plus a copy of a full 1206 x 2622 frame took 0.31 ms a frame here (a debug build,
  `a_full_frame_copies_inside_the_cpu_budget`), far inside the CPU path's 33 ms.
- The zero-copy import on a real ring: `a_real_simulator_ring_imports_with_no_copy` (opt-in, `FLARE_DEVICE_REAL=1`)
  booted a simulator made for it and showed 5 ring frames (1206 x 2622) as Slint images on the GPU path, no refusal.
- 8: 1317 was taken by open PR #293, so this plan's 1317 is **1323**; 1311, 1312 (Flare halves) and 1319 are in.
- 12: not measured in Flare yet: it needs the staged build's `FLARE_TRACE_FRAME=1` run with the probe app, which
  needs the owner's screen (see PR 4b "Before merge").
