# Celeste for Android (experimental)

The same app as on the desktop, built from the same crate. The phone layout switches on below 600 logical pixels of window width.

## How it runs

- `CelesteApplication` runs first in every process. It points the XDG and temp directories at the app's private storage and loads `libceleste.so`.
- `SyncService` is a foreground service with a silent notification that shows the sync status. It keeps the process, and with it the sync engine (`src/engine`), alive while no window is open. The engine starts with the app, after a reboot (`BootReceiver`, unless autostart is off in the preferences), after an update, and when Android restarts the service.
- `CelesteActivity` shows the iced GUI ([iced_android](https://github.com/Santuzius/iced_android)). Its window closes while Celeste is not on screen; syncing goes on.
- Credentials are encrypted with a Keystore key (`Bridge.encrypt`/`decrypt`). Folders are picked with the system's folder picker, and syncing them needs "All files access".

## Building

Needs rustup with the Android targets (`rustup target add aarch64-linux-android`), Nix, and a phone with USB debugging.

    scripts/android-dev.sh              # build, install, start (arm64)
    ABI=x86_64 scripts/android-dev.sh   # for the emulator
    LIB_ONLY=1 scripts/android-dev.sh   # only the native libraries

The script builds `libceleste.so` with cargo-ndk and the Go part as `libceleste_go.so`: Go cannot link a c-archive into an Android library, so it is a shared library there. It then builds the debug APK with Gradle from `android/shell.nix` and installs it. The debug APK carries the optimised native libraries; being debuggable, its files can be reached with `adb shell run-as io.github.santuzius.celeste`.

iced, iced_android and android-activity come from forks on GitHub (`Cargo.toml`); the android-activity fork lets the app outlive a destroyed activity. To work on a fork, point Cargo at its local checkout with a `[patch]` section in `.cargo/config.toml`, which is not committed, and don't commit the `Cargo.lock` that results.

## Release

    scripts/android-release.sh          # temp/release/Celeste-<version>-arm64-v8a.apk

The release APK is signed with the release key. `android/keystore.properties` (not committed) names it:

    storeFile=/home/you/.android/celeste-release.jks
    storePassword=…
    keyAlias=celeste
    keyPassword=…

Android installs an update only over an APK signed with the same key, so keep a backup of the keystore and its password; without them no update of the published app is possible. The versionCode follows from the version in `Cargo.toml` (0.21.0 → 21000).
