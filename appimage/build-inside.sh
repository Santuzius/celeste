#!/usr/bin/env bash
# Runs inside the container of appimage/Dockerfile: builds Syncative from the source copy in /src and packs /out/Syncative-<version>-x86_64.AppImage.
set -euo pipefail
export RUSTC_BOOTSTRAP=1 CARGO_TARGET_DIR=/cache/target GOCACHE=/cache/go-build GOMODCACHE=/cache/go-mod
export CARGO_HOME=/cache/cargo PATH="/opt/cargo/bin:$PATH" RUSTUP_HOME=/opt/rustup
cd /src
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
cargo build --release --locked
app=/cache/AppDir
rm -rf "$app"
install -Dm755 "$CARGO_TARGET_DIR/release/syncative" "$app/usr/bin/syncative"
install -Dm755 appimage/AppRun "$app/AppRun"
install -Dm644 appimage/syncative.desktop "$app/syncative.desktop"
install -Dm644 appimage/syncative.desktop "$app/usr/share/applications/syncative.desktop"
install -Dm644 assets/syncative-icon.svg "$app/syncative-icon.svg"
install -Dm644 assets/syncative-icon.svg "$app/usr/share/icons/hicolor/scalable/apps/syncative-icon.svg"
cd /out
# The update information lets AppImageUpdate and Gear Lever fetch newer releases from GitHub; appimagetool writes the matching .zsync next to it.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 /opt/appimagetool --no-appstream -u "gh-releases-zsync|Santuzius|syncative|latest|Syncative-*-x86_64.AppImage.zsync" "$app" "Syncative-$version-x86_64.AppImage"
