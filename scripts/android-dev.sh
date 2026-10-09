#!/usr/bin/env bash
# Builds Celeste for Android (arm64 unless ABI is set), installs it on the USB phone and starts it. Needs rustup with the Android targets and android/shell.nix (NDK, SDK, Gradle).
#   scripts/android-dev.sh            build, install, start
#   ABI=x86_64 scripts/android-dev.sh for the emulator
#   LIB_ONLY=1 scripts/android-dev.sh only builds the native libraries
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$(ls -d /nix/store/*-android-sdk-ndk-28.2.13676358/libexec/android-sdk/ndk/28.2.13676358 | head -1)}"
abi="${ABI:-arm64-v8a}"
log="$REPO/temp/android-cargo.log"
cd "$REPO/android"
rm -rf app/src/main/jniLibs
case "$abi" in arm64-v8a) triple=aarch64-linux-android ;; armeabi-v7a) triple=armv7-linux-androideabi ;; x86_64) triple=x86_64-linux-android ;; x86) triple=i686-linux-android ;; esac
# Go and libclang (for bindgen) come from the desktop dev shell; bindgen gets the NDK's headers instead of the host's.
read -r go_bin libclang < <(nix-shell "$REPO/shell.nix" --run 'echo "$(dirname "$(command -v go)") $LIBCLANG_PATH"')
export LIBCLANG_PATH="$libclang"
export "BINDGEN_EXTRA_CLANG_ARGS_${triple//-/_}=--sysroot=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot"
# The dev shell's mold flags are for the desktop linker only.
if ! env -u RUSTFLAGS -u BINDGEN_EXTRA_CLANG_ARGS PATH="$HOME/.cargo/bin:$go_bin:$PATH" nix shell nixpkgs#cargo-ndk -c cargo ndk --platform 26 -t "$abi" -o app/src/main/jniLibs --manifest-path "$REPO/Cargo.toml" build --lib --release > "$log" 2>&1; then
  grep -E '^error' -A14 "$log" | head -80; exit 1
fi
# The Go part is a shared library on Android (see src/go/build.rs); cargo-ndk only copies the cdylib.
go_so="$(find "$REPO/target/$triple/release/build" -name libceleste_go.so -printf '%T@ %p\n' | sort -n | tail -1 | cut -d' ' -f2)"
cp "$go_so" "app/src/main/jniLibs/$abi/"
[ -n "${LIB_ONLY:-}" ] && exit 0
nix-shell shell.nix --run 'gradle --quiet assembleRelease' 2>&1 | grep -v 'making symlink' || true
adb ${SERIAL:+-s $SERIAL} install -r app/build/outputs/apk/release/app-release.apk >/dev/null
adb ${SERIAL:+-s $SERIAL} logcat -c
adb ${SERIAL:+-s $SERIAL} shell am start -n io.github.santuzius.celeste/.CelesteActivity >/dev/null
