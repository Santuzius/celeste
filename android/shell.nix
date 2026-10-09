# Nix shell for building the Android app (scripts/android-dev.sh): Android SDK, NDK, JDK and Gradle. Rust comes from rustup, whose per-target standard libraries cargo-ndk needs: `rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android`.
{ pkgs ? import <nixpkgs> {
    config = {
      allowUnfree = true;
      android_sdk.accept_license = true;
    };
  }
}:

let
  # Highest API level the Gradle builds compile against; the minimum (26, Android 8.0) is set in each app/build.gradle.
  compileSdk = "36";
  buildTools = "36.0.0";
  # NDK r28+ aligns native libraries to 16 KB pages by default, which Android 15+ devices may require.
  ndk = "28.2.13676358";

  android = pkgs.androidenv.composeAndroidPackages {
    # Only the platform the app compiles against; emulator images live in the iced_android repository.
    platformVersions = [ compileSdk ];
    buildToolsVersions = [ buildTools ];
    includeNDK = true;
    ndkVersions = [ ndk ];
    includeEmulator = false;
    includeSystemImages = false;
    systemImageTypes = [ "default" "google_apis" ];
    abiVersions = [ "x86_64" ];
  };
  sdk = android.androidsdk;
in
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    sdk
    jdk17
    gradle
    cargo-ndk
  ];

  ANDROID_HOME = "${sdk}/libexec/android-sdk";
  ANDROID_SDK_ROOT = "${sdk}/libexec/android-sdk";
  ANDROID_NDK_HOME = "${sdk}/libexec/android-sdk/ndk/${ndk}";
  ANDROID_NDK_ROOT = "${sdk}/libexec/android-sdk/ndk/${ndk}";
  JAVA_HOME = pkgs.jdk17.home;
  # The Android Gradle plugin downloads aapt2 from Maven as a dynamically linked binary that NixOS cannot run; use the SDK's patched one.
  GRADLE_OPTS = "-Dorg.gradle.project.android.aapt2FromMavenOverride=${sdk}/libexec/android-sdk/build-tools/${buildTools}/aapt2";
}
