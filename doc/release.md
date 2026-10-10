# Making a release

A release ships three things for x86_64 Linux and arm64 Android:

- the signed APK, which Obtainium picks up
- the AppImage with its `.zsync` file for updates
- the Nix package in the binary cache `celeste.cachix.org`

The branch flow:

- Work happens on `develop` or a feature branch.
- `main` is GitHub's default branch and always points at the latest release. The README sends Google's consent screen to `PRIVACY.md` on `main`, and `github:Santuzius/celeste` builds from it. Move `main` only at releases: every commit changes the source, and with it the package hash, so the cached package would no longer match.
- Tags are `vX.Y.Z`.

## Prerequisites

- **Clean `Cargo.lock`:**
  - Without `.cargo/config.toml`: its `[patch]` sections point Cargo at local checkouts of the forks and put path sources into `Cargo.lock`.
  - The forks (iced, iced_android, android-activity) must be pushed. Nix and the AppImage build fetch them from GitHub.
- **Docker** for the AppImage, **rustup with the Android targets** and the Nix shells for the APK (see [android/README.md](../android/README.md)).
- **`android/keystore.properties`** with the release key (below).
- **A Cachix auth token** for the cache `celeste` with write access, passed as `CACHIX_AUTH_TOKEN`. Cachix's web interface issues them.

## Steps

1. **Bump** the version in `[workspace.package]` of `Cargo.toml` in a commit of its own (`:bookmark: Bump crate to X.Y.Z`). Then run `cargo metadata` in the dev shell so `Cargo.lock` follows; it belongs in the same commit.
2. **Tag** the bump commit (`git tag -a vX.Y.Z -m "Celeste X.Y.Z"`) and move `develop` and `main` to it.
3. **Build the APK and the AppImage** from the clean tree. Both write to `temp/release/`:
   ```sh
   scripts/android-release.sh    # Celeste-X.Y.Z-arm64-v8a.apk
   scripts/build-appimage.sh     # Celeste-X.Y.Z-x86_64.AppImage and .zsync
   ```
4. **Push** the branches and the tag: `git push origin develop main vX.Y.Z`.
5. **Build the Nix package from the pushed tag** and push it to the cache:
   ```sh
   nix build github:Santuzius/celeste/vX.Y.Z#celeste -o temp/result-release
   cachix push celeste $(readlink temp/result-release)
   ```
   Build it from GitHub, not from the local checkout (`.#celeste`). The local flake source leaves out the empty submodule directory `src/go/proton-api`, which GitHub's archive contains. The source then differs, and so does the hash of the package. A cache filled from `.` is never asked for; this happened with 0.21.0 and 0.21.1.
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
- **Nix:** `nix eval --raw github:Santuzius/celeste/vX.Y.Z#celeste.outPath` equals the path pushed to Cachix, and `nix path-info --store https://celeste.cachix.org <path>` finds it.
- **Downloads:** The release assets downloaded with `gh release download` match `SHA256SUMS`.

## The APK release key

- Android installs an update only over an APK signed with the same key. If the key is lost, users have to uninstall and lose their sign-ins, and Obtainium stops updating. Keep the keystore backed up and its password in a password manager.
- `android/keystore.properties`, not committed, names the key; `android/app/build.gradle` reads it. Without the file the release APK stays unsigned and is not published by mistake with the debug key.
- The versionCode follows from the version: 0.21.1 → 21001.
- Releases up to 0.20.0 had no APK. Development builds are signed with each computer's debug key and have to be uninstalled once before the release APK installs.

## The Nix binary cache

- The flake offers the cache through `nixConfig`, for `nix profile`, `nix run` and `nix build`. Nix honours it only for users in the daemon's `trusted-users`.
- The NixOS module adds it to `nix.settings` (`programs.celeste.binaryCache`, on by default). It takes effect from the rebuild after the one that enables it. For a first rebuild that already uses the cache, pass `--option extra-substituters https://celeste.cachix.org --option extra-trusted-public-keys celeste.cachix.org-1:iGmU8GUPr4AlGAiwmfAIDRhRKZwpIpWcz7DKwQBaRm8=` to `sudo nixos-rebuild`.
- The cached package belongs to the nixpkgs in Celeste's `flake.lock`. A system that sets `inputs.celeste.inputs.nixpkgs.follows` builds Celeste itself.
- Only x86_64-linux is cached; aarch64-linux builds from source.

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
