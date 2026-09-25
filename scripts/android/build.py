#!/usr/bin/env python3
"""Build the real Rust/Compose Android client on the development host.

Pinned SDK packages: platforms;android-36, build-tools;36.0.0,
ndk;28.2.13676358. Uses a normal installed Android Rust target, or build-std
with the host compiler's matching rust-src (Homebrew Rust on mini1).
"""
import argparse
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]

sys.path.insert(0, str(ROOT / 'scripts/lib'))
from build_env import build_environment
APP = ROOT / "apps/android"
TARGET = "aarch64-linux-android"
NDK_VERSION = "28.2.13676358"


def run(command, env, **kwargs):
    print("+", " ".join(str(x) for x in command), flush=True)
    return subprocess.run(command, cwd=ROOT, env=env, check=True, **kwargs)


def verify_jni(library, llvm, env):
    """Reject a package whose native exports no longer match the Kotlin namespace."""
    symbols = subprocess.check_output([str(llvm / "llvm-nm"), "-D", "--defined-only", str(library)], env=env, text=True)
    expected = []
    for source in (APP / "app/src/main/java").rglob("*.kt"):
        text = source.read_text()
        package = re.search(r"^package ([\w.]+)", text, re.M)
        if not package:
            continue
        for owner, body in re.findall(r"(?:internal )?object (\w+)\s*\{(.*?)\n\}", text, re.S):
            for method in re.findall(r"external fun (\w+)\s*\(", body):
                expected.append("Java_" + package[1].replace(".", "_") + "_" + owner + "_" + method)
    missing = [symbol for symbol in expected if not re.search(r"\b" + re.escape(symbol) + r"$", symbols, re.M)]
    if missing:
        raise SystemExit("Missing packaged JNI exports: " + ", ".join(missing))
    print(f"Verified {len(expected)} Kotlin/native JNI bindings", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--channel", choices=("test", "dev"), default="test")
    parser.add_argument("--skip-native", action="store_true")
    parser.add_argument("--native-only", action="store_true")
    parser.add_argument("--tests", action="store_true", help="also package instrumentation tests")
    parser.add_argument("--profile", action="store_true", help="build a non-debuggable APK with profiling enabled, retaining the selected channel app ID/signature")
    args = parser.parse_args()
    env = build_environment(variant='android')
    sdk = Path(env.get("ANDROID_HOME", Path.home() / "Library/Android/sdk"))
    ndk = sdk / "ndk" / NDK_VERSION
    host = "darwin-x86_64" if sys.platform == "darwin" else "linux-x86_64"
    llvm = ndk / "toolchains/llvm/prebuilt" / host / "bin"
    compiler = llvm / "aarch64-linux-android29-clang"
    if not compiler.exists():
        raise SystemExit(f"Missing NDK: install ndk;{NDK_VERSION} in {sdk}")
    env.update(ANDROID_HOME=str(sdk), ANDROID_NDK_HOME=str(ndk),
               CARGO_INCREMENTAL="0", CARGO_PROFILE_DEV_DEBUG="0", CARGO_BUILD_JOBS="4",
               CC_aarch64_linux_android=str(compiler),
               CXX_aarch64_linux_android=str(compiler) + "++",
               AR_aarch64_linux_android=str(llvm / "llvm-ar"),
               CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=str(compiler),
               CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384")
    # Native QuickJS bindings must use the Android headers and ABI, including
    # when libclang itself is provided by the host toolchain.
    env.setdefault("BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android",
                   f"--target=aarch64-linux-android29 --sysroot={llvm.parent / 'sysroot'}")
    # Do not contend with desktop Cargo's build lock or change its feature graph.
    env.setdefault("CARGO_TARGET_DIR", str(ROOT / "target/android"))
    if not env.get("JAVA_HOME"):
        jdk = Path("/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home")
        if jdk.exists():
            env["JAVA_HOME"] = str(jdk)
    if env.get("JAVA_HOME"):
        env["PATH"] = str(Path(env["JAVA_HOME"]) / "bin") + os.pathsep + env["PATH"]

    if not args.skip_native:
        target_lib = Path(subprocess.check_output(
            ["rustc", "--print", "target-libdir", "--target", TARGET], env=env, text=True).strip())
        command = ["cargo", "build", "--locked", "-p", "zork-android", "--target", TARGET]
        if not any(target_lib.glob("libstd-*.rlib")):
            sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], env=env, text=True).strip())
            if not (sysroot / "lib/rustlib/src/rust/library/Cargo.toml").exists():
                raise SystemExit(f"Install Rust target {TARGET} or the matching rust-src component")
            env["RUSTC_BOOTSTRAP"] = "1"
            command.append("-Zbuild-std=std,panic_abort")
        run(command, env)

    build = APP / "app/build/generated"
    native = build / "jniLibs/arm64-v8a"
    native.mkdir(parents=True, exist_ok=True)
    cargo_target = Path(env.get("CARGO_TARGET_DIR", ROOT / "target"))
    shutil.copy2(cargo_target / TARGET / "debug/libzork_android.so", native)
    # Strip only the staged APK copy; retain build symbols for diagnosis.
    run([str(llvm / "llvm-strip"), "--strip-unneeded", str(native / "libzork_android.so")], env)
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", TARGET],
        cwd=ROOT, env=env, text=True))
    verify_jni(native / "libzork_android.so", llvm, env)
    # The vendored compatibility patch preserves the exact upstream JNI ABI.
    versions = {p["name"]: p["version"] for p in metadata["packages"]
                if p["name"] in ("rustls-platform-verifier", "rustls-platform-verifier-android")}
    if versions != {"rustls-platform-verifier": "0.7.0", "rustls-platform-verifier-android": "0.1.1"}:
        raise SystemExit("Review the vendored Android certificate verifier before changing its JNI ABI")
    if not args.native_only:
        tasks = ["-PzorkChannel=" + args.channel, "assembleDebug"] + (["assembleDebugAndroidTest"] if args.tests else [])
        if args.profile:
            tasks.append("-PzorkProfile=true")
        run([str(APP / "gradlew"), "-p", str(APP), "--no-daemon", *tasks], env)
        print(APP / "app/build/outputs/apk/debug/app-debug.apk")
    from build_maintenance import after_build
    after_build(env)


if __name__ == "__main__":
    main()
