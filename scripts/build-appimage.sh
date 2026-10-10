#!/usr/bin/env bash
# Builds temp/release/Celeste-<version>-x86_64.AppImage in an Ubuntu 22.04 container (appimage/Dockerfile), so it runs on distributions with glibc 2.35 or newer. Needs Docker. Cargo, Go and the build directory are cached in the Docker volume celeste-appimage-cache.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
out="$REPO/temp/release"
src="$REPO/temp/appimage-src"
mkdir -p "$out"
docker build -q -t celeste-appimage "$REPO/appimage" >/dev/null
# A copy of the working tree without build output, including the proton-api submodule.
rm -rf "$src"
mkdir -p "$src"
tar -C "$REPO" --exclude=./target --exclude=./temp --exclude=./result --exclude=./android --exclude=./.git --exclude=./.cargo/config.toml -cf - . | tar -C "$src" -xf -
docker volume create celeste-appimage-cache >/dev/null
docker run --rm -v celeste-appimage-cache:/cache alpine chown "$(id -u):$(id -g)" /cache
docker run --rm --user "$(id -u):$(id -g)" -e HOME=/cache -v celeste-appimage-cache:/cache -v "$src:/src:ro" -v "$out:/out" celeste-appimage /opt/build-inside.sh
rm -rf "$src"
ls -l "$out"/Celeste-*-x86_64.AppImage*
