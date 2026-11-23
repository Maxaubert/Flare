# Android agent helper

`flare-agent.jar` reads the UI tree of an Android emulator and types text into its fields, for the device panel
and the agent's `device_*` tools (Android plan PR A3, rule 1369). It is Flare's own code under Flare's licence.

## What it is

- `src/dev/flare/agent/`: Java, about 500 lines. `Agent.java` holds one `UiAutomation` connection and answers JSON
  lines on the abstract socket `flare-agent`. `Tree.java` reads every window. `Text.java` types at the cursor.
- The emu host (`crates/flare-emu-host/src/agent.rs`) pushes the jar to `/data/local/tmp/flare-agent-<hash>.jar`,
  starts it with `app_process` as the shell user, and reaches it through `adb forward tcp:0
  localabstract:flare-agent`. Only an `emulator-<port>` serial gets it, never a real phone.
- Only the shell user (adb's forward) or root may connect. Another app in the device is refused.
- It ends by itself 15 s after its last client leaves, so it frees Android's single UI automation connection.

Requests: `hello`, `tree`, `set_text {text, mode: insert|replace, at?: [x, y]}`, `front_app`, `quit`. Events:
`{"event":"windows","count":n}` at most ten times a second.

## Build

Needs a JDK (11 or newer) and an Android SDK with build-tools (`d8`) and a platform (`android.jar`). The SDK is
only read.

```
python tools/android-agent/build.py           # writes crates/flare-device/assets/flare-agent.jar and .sha256
python tools/android-agent/build.py --check   # is the checked-in jar from these sources?
```

`flare-agent.jar.sha256` holds the jar's SHA-256 and the hash of `src/` plus `build.py`. The test
`the_checked_in_jar_is_built_from_its_sources` (`flare-device`, `emu::agent`) checks both on every CI run, with no
Java. So after any change in `src/` or `build.py`, run the build and commit the new jar. The build is
reproducible: the same sources give the same jar.

No lambdas or method references in the source: `android.jar` has no `LambdaMetafactory` for `javac` to link.

## By hand

With a running emulator (made for the check, headless):

```
flare-emu-host --agent emulator-5554 --text "blåbær 😀"
```

It starts the helper, times a first tree and 20 warm ones, types the text into the focused field, runs
`uiautomator dump` once for comparison, and stops. On API 37 (2026-11-22): start 350 ms, first tree 70 to 90 ms,
warm tree p50 about 9 ms, the dump 2.2 s.

## Probe app

`probe/` is a small app for the frame and touch checks (Step 0 S1 and S2, the bench, the opt-in real e2e). It is
never shipped and its build output is not checked in.

```
python tools/android-agent/probe/build.py     # writes probe/out/flare-probe.apk
adb -s emulator-5554 install -r -t tools/android-agent/probe/out/flare-probe.apk
adb -s emulator-5554 shell am start -n dev.flare.probe/.Probe       # barcode frame counter, touch band
adb -s emulator-5554 shell am start -n dev.flare.probe/.TextProbe   # a text field for set-text
```
