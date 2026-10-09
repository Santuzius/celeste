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

Until the latest commits of the iced and iced_android forks are pushed and the android-activity fork (which lets the app outlive a destroyed activity) is published, these come from local checkouts through `.cargo/config.toml`, which is not committed.
