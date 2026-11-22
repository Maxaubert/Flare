"""Builds Flare's agent helper, flare-agent.jar (Android plan PR A3, rule 1369).

    python tools/android-agent/build.py            build, then write the jar and its hashes
    python tools/android-agent/build.py --check    only say whether the checked-in jar is from these sources

It compiles tools/android-agent/src with javac against the SDK's android.jar, turns the classes into dex with the
SDK's d8, and writes crates/flare-device/assets/flare-agent.jar plus flare-agent.jar.sha256, which holds the jar's
SHA-256 and the hash of the sources it came from. A test in flare-device (emu::agent) checks both on every CI run, so
CI needs no Java: change a source file, run this script, commit the new jar.

Needs a JDK (javac, 11 or newer) and an Android SDK with build-tools (d8) and a platform (android.jar). It only reads
the SDK. Python standard library only.
"""

import hashlib
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
SRC = HERE / "src"
ASSETS = REPO / "crates" / "flare-device" / "assets"
JAR = ASSETS / "flare-agent.jar"
SUMS = ASSETS / "flare-agent.jar.sha256"
MIN_API = "29"


def hashed_files():
    """What the source hash covers: every file under src, and this script (its flags shape the jar)."""
    files = sorted(p for p in SRC.rglob("*") if p.is_file())
    return [(p.relative_to(HERE).as_posix(), p) for p in files] + [("build.py", Path(__file__).resolve())]


def source_sha256():
    """SHA-256 over each file's path, a newline, its bytes without CR, a newline, in path order. CR is dropped so a
    Windows checkout hashes the same as a Mac one. flare_device::emu::agent::source_sha256 is the same sum."""
    h = hashlib.sha256()
    for rel, p in sorted(hashed_files()):
        h.update(rel.encode() + b"\n")
        h.update(p.read_bytes().replace(b"\r", b"") + b"\n")
    return h.hexdigest()


def file_sha256(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()


def read_sums():
    out = {}
    if SUMS.exists():
        for line in SUMS.read_text().splitlines():
            k, _, v = line.partition(" ")
            out[k] = v.strip()
    return out


def sdk_root():
    for k in ("ANDROID_HOME", "ANDROID_SDK_ROOT"):
        v = os.environ.get(k)
        if v and Path(v).is_dir():
            return Path(v)
    if sys.platform == "win32":
        p = Path(os.environ.get("LOCALAPPDATA", "")) / "Android" / "Sdk"
    elif sys.platform == "darwin":
        p = Path.home() / "Library" / "Android" / "sdk"
    else:
        p = Path.home() / "Android" / "Sdk"
    if p.is_dir():
        return p
    sys.exit("no Android SDK: set ANDROID_HOME")


def version_key(name):
    parts = []
    for x in name.replace("android-", "").replace("-", ".").split("."):
        parts.append(int(x) if x.isdigit() else -1)
    return parts


def newest(dirs):
    dirs = [d for d in dirs if d is not None]
    return max(dirs, key=lambda d: version_key(d.name)) if dirs else None


def tools(sdk):
    d8_name = "d8.bat" if sys.platform == "win32" else "d8"
    bt = newest([d for d in (sdk / "build-tools").glob("*") if (d / d8_name).exists()])
    plat = newest([d for d in (sdk / "platforms").glob("android-*") if (d / "android.jar").exists()])
    if bt is None or plat is None:
        sys.exit("the SDK needs build-tools (d8) and a platform (android.jar)")
    javac = shutil.which("javac")
    jh = os.environ.get("JAVA_HOME")
    if jh:
        cand = Path(jh) / "bin" / ("javac.exe" if sys.platform == "win32" else "javac")
        if cand.exists():
            javac = str(cand)
    if not javac:
        sys.exit("no javac: install a JDK (11 or newer) or set JAVA_HOME")
    return javac, bt / d8_name, plat / "android.jar"


def build():
    sdk = sdk_root()
    javac, d8, android_jar = tools(sdk)
    print(f"javac {javac}\nd8 {d8}\nandroid.jar {android_jar}")
    sources = [str(p) for p in sorted(SRC.rglob("*.java"))]
    with tempfile.TemporaryDirectory(prefix="flare-agent-") as tmp:
        classes = Path(tmp) / "classes"
        classes.mkdir()
        subprocess.run(
            [javac, "-source", "8", "-target", "8", "-Xlint:-options", "-encoding", "UTF-8",
             "-bootclasspath", str(android_jar), "-d", str(classes)] + sources,
            check=True,
        )
        out = Path(tmp) / "flare-agent.jar"
        class_files = [str(p) for p in sorted(classes.rglob("*.class"))]
        subprocess.run(
            [str(d8), "--release", "--min-api", MIN_API, "--lib", str(android_jar), "--output", str(out)]
            + class_files,
            check=True,
        )
        ASSETS.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(out, JAR)
    SUMS.write_text(f"jar {file_sha256(JAR)}\nsource {source_sha256()}\n", newline="\n")
    print(f"wrote {JAR} ({JAR.stat().st_size} bytes)\n{SUMS.read_text()}", end="")


def check():
    sums = read_sums()
    ok = JAR.exists() and sums.get("jar") == file_sha256(JAR) and sums.get("source") == source_sha256()
    print("the jar is from these sources" if ok else "the jar is stale: run python tools/android-agent/build.py")
    return 0 if ok else 1


if __name__ == "__main__":
    if "--check" in sys.argv[1:]:
        sys.exit(check())
    build()
