"""Builds Flare's probe app (tools/android-agent/probe/out/flare-probe.apk). Never shipped with Flare and never checked
in: it is for the frame and touch checks of Step 0 (S1, S2), the bench and the opt-in real e2e scenario.

    python tools/android-agent/probe/build.py

Two activities: `.Probe` paints a frame counter as a barcode every vsync and flips a band on each touch;
`.TextProbe` holds a text field for set-text checks. No Gradle: aapt2 links the manifest, javac and d8 make the dex,
zipalign and apksigner finish the apk with a debug key made in out/ (keytool). Needs a JDK and an Android SDK with
build-tools and a platform; it only reads the SDK. Python standard library only.
"""

import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.dont_write_bytecode = True  # no __pycache__ beside the helper's build script
sys.path.insert(0, str(HERE.parent))
import build as agent_build  # noqa: E402  (the helper's build script: SDK and JDK lookup)

OUT = HERE / "out"


def tool(bt, name):
    for n in (name + ".bat", name + ".exe", name):
        if (bt / n).exists():
            return str(bt / n)
    sys.exit(f"build-tools {bt.name} has no {name}")


def main():
    sdk = agent_build.sdk_root()
    javac, d8, android_jar = agent_build.tools(sdk)
    bt = Path(d8).parent
    keytool = shutil.which("keytool")
    if not keytool:
        keytool = str(Path(javac).parent / ("keytool.exe" if sys.platform == "win32" else "keytool"))
    if OUT.exists():
        shutil.rmtree(OUT)
    (OUT / "classes").mkdir(parents=True)
    run = lambda *a: subprocess.run([str(x) for x in a], check=True)  # noqa: E731
    base = OUT / "base.apk"
    run(tool(bt, "aapt2"), "link", "--manifest", HERE / "AndroidManifest.xml", "-I", android_jar, "-o", base)
    sources = sorted((HERE / "src").rglob("*.java"))
    run(javac, "-source", "8", "-target", "8", "-Xlint:-options", "-encoding", "UTF-8",
        "-bootclasspath", android_jar, "-d", OUT / "classes", *sources)
    run(d8, "--release", "--min-api", "29", "--lib", android_jar, "--output", OUT,
        *sorted((OUT / "classes").rglob("*.class")))
    with zipfile.ZipFile(base, "a") as z:
        z.write(OUT / "classes.dex", "classes.dex")
    aligned = OUT / "aligned.apk"
    run(tool(bt, "zipalign"), "-f", "4", base, aligned)
    key = OUT / "debug.keystore"
    run(keytool, "-genkeypair", "-keystore", key, "-storepass", "android", "-keypass", "android", "-alias", "probe",
        "-keyalg", "RSA", "-keysize", "2048", "-validity", "3650", "-dname", "CN=Flare probe", "-noprompt")
    apk = OUT / "flare-probe.apk"
    run(tool(bt, "apksigner"), "sign", "--ks", key, "--ks-pass", "pass:android", "--out", apk, aligned)
    print(f"wrote {apk}")
    print("install: adb -s emulator-5554 install -r -t " + str(apk))
    print("start:   adb -s emulator-5554 shell am start -n dev.flare.probe/.Probe   (or .TextProbe)")


if __name__ == "__main__":
    main()
