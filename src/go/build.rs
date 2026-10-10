use std::{env, path::PathBuf, process::Command};

/// Build `libceleste_go.a` + `libceleste_go.h` from the Go code
/// in this crate's directory, then feed the header through bindgen to
/// produce Rust FFI declarations. Mirrors librclone-sys's pattern, with
/// the addition of our `ProtonDrive_*` surface and the drop of rclone's
/// protondrive backend (see wrapper.go).
fn main() {
    let target_triple = env::var("TARGET").expect("TARGET not set");
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // docs.rs has no network; bail out with empty bindings so the doc
    // build on crates.io (if we ever publish) doesn't need Go.
    if env::var("DOCS_RS").is_ok() {
        std::fs::write(out_dir.join("bindings.rs"), "").unwrap();
        return;
    }

    // Re-run when the Go module graph changes or our shim is edited.
    println!("cargo:rerun-if-changed=go.mod");
    println!("cargo:rerun-if-changed=go.sum");
    println!("cargo:rerun-if-changed=wrapper.go");
    println!("cargo:rerun-if-changed=authorize.go");
    println!("cargo:rerun-if-changed=configpass.go");
    // The proton-api submodule is the real source of truth for that
    // dependency; its HEAD commit is tracked by git submodule, but
    // when we edit files there we want a rebuild.
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("proton-api").display()
    );
    // The native Drive layer lives under drive/; rebuild when any
    // file there changes.
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("drive").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("proton-ext").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("rclonewatch").display()
    );

    // Go supports c-archive on Android only for its own runtime, not for linking into a foreign binary, so Android gets a shared library that has to ship next to libceleste.so.
    let android = target_triple.contains("android");
    let (build_mode, lib_path) = if android {
        ("-buildmode=c-shared", out_dir.join("libceleste_go.so"))
    } else {
        ("-buildmode=c-archive", out_dir.join("libceleste_go.a"))
    };
    let header_path = out_dir.join("libceleste_go.h");

    let mut go = Command::new("go");
    go.current_dir(&manifest_dir).args(["build", build_mode]);
    if android {
        cross_compile_for_android(&mut go, &target_triple);
    }
    go.arg("-o").arg(&lib_path).arg(".");
    let status = go
        .status()
        .expect("`go build` failed. Is `go` installed and latest version?");
    assert!(status.success(), "go build failed");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    // Rust strips the `lib` prefix and `.a` suffix before passing to the
    // linker, so `celeste_go` resolves to `libceleste_go.a`.
    if android {
        println!("cargo:rustc-link-lib=dylib=celeste_go");
    } else {
        println!("cargo:rustc-link-lib=static=celeste_go");
    }

    // librclone-sys links these frameworks on macOS; mirror for parity.
    if target_triple.ends_with("darwin") {
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=IOKit");
        println!("cargo:rustc-link-lib=framework=Security");
        println!("cargo:rustc-link-lib=resolv");
    }

    let bindings = bindgen::Builder::default()
        .header(header_path.to_string_lossy())
        // Only surface the symbols we intend to call from Rust — bindgen
        // would otherwise emit every `_GoString_` helper and friend,
        // which pollutes the Rust side.
        .allowlist_function("RcloneInitialize")
        .allowlist_function("RcloneFinalize")
        .allowlist_function("RcloneRPC")
        .allowlist_function("RcloneFreeString")
        .allowlist_function("RcloneRemoteChanged")
        .allowlist_function("RcloneForgetRemote")
        .allowlist_function("RcloneSetChangePollSeconds")
        .allowlist_function("CelesteAuthorize")
        .allowlist_function("CelesteAuthorizeURL")
        .allowlist_function("CelesteAuthorizeCancel")
        .allowlist_function("CelesteSetConfigPassword")
        .allowlist_function("CelesteSaveConfig")
        .allowlist_function("ProtonDrive_Version")
        .allowlist_function("ProtonDrive_SetAppVersion")
        .allowlist_function("ProtonDrive_Login")
        .allowlist_function("ProtonDrive_Logout")
        .allowlist_function("ProtonDrive_SaveSession")
        .allowlist_function("ProtonDrive_SaveSessionIfRotated")
        .allowlist_function("ProtonDrive_ResumeSession")
        .allowlist_function("ProtonDrive_RootLinkID")
        .allowlist_function("ProtonDrive_ListDirectory")
        .allowlist_function("ProtonDrive_ListRecursive")
        .allowlist_function("ProtonDrive_Stat")
        .allowlist_function("ProtonDrive_FileDetails")
        .allowlist_function("ProtonDrive_DownloadFile")
        .allowlist_function("ProtonDrive_CreateFolder")
        .allowlist_function("ProtonDrive_UploadFile")
        .allowlist_function("ProtonDrive_TrashLink")
        .allowlist_function("ProtonDrive_PermanentDeleteLink")
        .allowlist_type("RcloneRPCResult")
        // The header is our own output; watching it would mark the build script stale after every run and rebuild the Go archive each time.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new().rerun_on_header_files(false)))
        .generate()
        .expect("Unable to generate bindings");
    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("failed to write bindings.rs");
}

/// Builds the Go archive for Android with the NDK's clang, which cargo-ndk provides as `CC_<target>`.
fn cross_compile_for_android(go: &mut Command, target_triple: &str) {
    let goarch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64",
        "arm" => "arm",
        "x86_64" => "amd64",
        "x86" => "386",
        other => panic!("no Go architecture for {other}"),
    };
    let target_env = |prefix: &str| {
        let names = [format!("{prefix}_{}", target_triple.replace('-', "_")), format!("{prefix}_{target_triple}"), format!("TARGET_{prefix}")];
        for name in &names {
            println!("cargo:rerun-if-env-changed={name}");
        }
        names.iter().find_map(|name| env::var(name).ok())
    };
    let cc = target_env("CC").expect("building for Android needs the NDK's clang in CC_<target>; build with cargo-ndk");
    // cargo-ndk hands over the bare clang and the target (`--target=aarch64-linux-android26`) in CFLAGS; Go needs both in CC.
    let cc = match target_env("CFLAGS") {
        Some(flags) => format!("{cc} {flags}"),
        None => cc,
    };
    go.env("GOOS", "android").env("GOARCH", goarch).env("CGO_ENABLED", "1").env("CC", cc);
    // The soname lets Android's linker find the library in the APK's lib directory when it loads libceleste.so; -s -w leave out the symbol and debug tables, which only cost APK size.
    go.args(["-ldflags", "-s -w -extldflags=-Wl,-soname,libceleste_go.so"]);
    if goarch == "arm" {
        go.env("GOARM", "7");
    }
}
