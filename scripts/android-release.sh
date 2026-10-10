#!/usr/bin/env bash
# Builds the signed release APK for arm64 phones as temp/release/Celeste-<version>-arm64-v8a.apk. Needs the same tools as scripts/android-dev.sh plus android/keystore.properties with the release key (see android/app/build.gradle).
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
if [ ! -f "$REPO/android/keystore.properties" ]; then
  echo "android/keystore.properties is missing; the APK would not be signed." >&2; exit 1
fi
if [ -f "$REPO/.cargo/config.toml" ]; then
  echo "warning: .cargo/config.toml is present, so local checkouts replace the pushed forks." >&2
fi
ABI=arm64-v8a LIB_ONLY=1 "$REPO/scripts/android-dev.sh"
cd "$REPO/android"
nix-shell shell.nix --run 'gradle --quiet assembleRelease' 2>&1 | grep -v 'making symlink' || true
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/Cargo.toml" | head -1)"
mkdir -p "$REPO/temp/release"
cp app/build/outputs/apk/release/app-release.apk "$REPO/temp/release/Celeste-$version-arm64-v8a.apk"
ls -l "$REPO/temp/release/Celeste-$version-arm64-v8a.apk"
