# Making a release

A release ships three things for x86_64 Linux and arm64 Android:

- the signed APK, which Obtainium picks up
- the AppImage with its `.zsync` file for updates
- for Nix, the same program: the flake downloads it from the AppImage (`nix/package-bin.nix`), so nothing is compiled and no binary cache is needed

The branch flow:

- Work happens on `develop` or a feature branch.
- `main` is GitHub's default branch and always points at the latest release, so its README and `PRIVACY.md` (linked from Google's consent screen) describe what users get. `github:Santuzius/celeste` also reads `nix/release.json` from `main`.
- Tags are `vX.Y.Z`.

## Prerequisites

- **Clean `Cargo.lock`:**
  - Without `.cargo/config.toml`: its `[patch]` sections point Cargo at local checkouts of the forks and put path sources into `Cargo.lock`.
  - The forks (iced, iced_android, android-activity) must be pushed. Nix and the AppImage build fetch them from GitHub.
- **Docker** for the AppImage, **rustup with the Android targets** and the Nix shells for the APK (see [android/README.md](../android/README.md)).
- **`android/keystore.properties`** with the release key (below).

## Steps

1. **Bump** the version in `[workspace.package]` of `Cargo.toml`, then run `cargo metadata` in the dev shell so `Cargo.lock` follows. Don't commit yet.
2. **Build the APK and the AppImage** from this tree. Both write to `temp/release/`:
   ```sh
   scripts/android-release.sh    # Celeste-X.Y.Z-arm64-v8a.apk
   scripts/build-appimage.sh     # Celeste-X.Y.Z-x86_64.AppImage and .zsync
   ```
3. **Pin the AppImage for Nix:** put the version and the hash of exactly this file into `nix/release.json`:
   ```sh
   nix hash file temp/release/Celeste-X.Y.Z-x86_64.AppImage
   ```
   The flake downloads the AppImage from the release under this hash. Never rebuild the AppImage after this step: a new build has a different hash.
4. **Commit** `Cargo.toml`, `Cargo.lock` and `nix/release.json` together (`:bookmark: Bump crate to X.Y.Z`). Tag that commit (`git tag -a vX.Y.Z -m "Celeste X.Y.Z"`) and move `develop` and `main` to it.
5. **Push** the branches and the tag: `git push origin develop main vX.Y.Z`.
6. **Release** with notes in the style of the earlier ones (Highlights, Fixes, Other):
   ```sh
   cd temp/release
   sha256sum Celeste-X.Y.Z-arm64-v8a.apk Celeste-X.Y.Z-x86_64.AppImage > SHA256SUMS
   gh release create vX.Y.Z --verify-tag --latest --title "Celeste X.Y.Z" --notes-file notes.md \
     Celeste-X.Y.Z-arm64-v8a.apk Celeste-X.Y.Z-x86_64.AppImage Celeste-X.Y.Z-x86_64.AppImage.zsync SHA256SUMS
   ```

## Checks before announcing it

- **APK:**
  - `apksigner verify --print-certs` shows the release certificate, with SHA-256 `6dd390bb5a6ffc6abb6a71620688550927d9f5b883ca076414388e3aa52f8743`.
  - `aapt2 dump badging` shows the new versionCode.
- **AppImage:** It runs `--help` in `ubuntu:24.04`, `debian:12` and `fedora:42` containers (with `--appimage-extract-and-run` and the distribution's dbus library). It needs nothing but glibc 2.35 and `libdbus-1`.
- **Nix:** `nix build github:Santuzius/celeste/vX.Y.Z` downloads the AppImage without compiling anything, and `result/bin/celeste --help` runs. This only works once the release is published, because the flake fetches the AppImage from it.
- **Downloads:** The release assets downloaded with `gh release download` match `SHA256SUMS`.

## The APK release key

- Android installs an update only over an APK signed with the same key. If the key is lost, users have to uninstall and lose their sign-ins, and Obtainium stops updating. Keep the keystore backed up and its password in a password manager.
- `android/keystore.properties`, not committed, names the key; `android/app/build.gradle` reads it. Without the file the release APK stays unsigned and is not published by mistake with the debug key.
- The versionCode follows from the version: 0.21.1 → 21001.
- Releases up to 0.20.0 had no APK. Development builds are signed with each computer's debug key and have to be uninstalled once before the release APK installs.

## The Nix package

- **On x86_64 (`default`, `celeste`):** `nix/package-bin.nix` takes the program out of the release's AppImage. `autoPatchelfHook` then points it at glibc, libdbus and libgcc from nixpkgs, which come prebuilt from `cache.nixos.org`.
  - Users need no binary cache of their own.
  - `inputs.nixpkgs.follows` doesn't matter.
- **From source:** `celeste-source` (or `programs.celeste.fromSource = true;`) builds from the checkout with `nix/package.nix`. On aarch64 every package builds from source, since there is no AppImage for it.
- **Why not a binary cache:** Nix uses a cache only if the administrator allows it (`trusted-users` or `/etc/nix/nix.conf`); a flake cannot add one by itself. 0.21.0 and 0.21.1 used Cachix (`celeste.cachix.org`, now unused), which only worked on NixOS through the module and without `follows`.

## The AppImage

- `appimage/Dockerfile` builds in Ubuntu 22.04, so glibc 2.35 is the minimum.
- `appimage/build-inside.sh` packs the AppDir with appimagetool's static runtime: no libfuse2 needed, only the system's `fusermount`.
- The update information (`gh-releases-zsync|Santuzius|celeste|latest|Celeste-*-x86_64.AppImage.zsync`) lets Gear Lever and AppImageUpdate find newer releases.
- Celeste writes the path of the `.AppImage` file (`$APPIMAGE`) into its autostart entry, because the binary inside lies in a mount point that changes with every start.
- Tested in a Kubuntu 24.04 VM, under X11 and Wayland:
  - Autostart after a reboot works.
  - The tray icon appears.
  - Celeste keeps running after the window closes.
  - A second start brings up the running instance's window.
