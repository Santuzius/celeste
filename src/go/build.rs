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

    let lib_path = out_dir.join("libceleste_go.a");
    let header_path = out_dir.join("libceleste_go.h");

    let status = Command::new("go")
        .current_dir(&manifest_dir)
        .args(["build", "-buildmode=c-archive", "-o"])
        .arg(&lib_path)
        .arg(".")
        .status()
        .expect("`go build` failed. Is `go` installed and latest version?");
    assert!(status.success(), "go build failed");

    // Mirror the freshly-built archive into the crate's manifest dir.
    // The Nix package can't run `go build` (sandboxed, no network) so
    // it copies `manifest_dir/libceleste_go.{a,h}` into its own
    // OUT_DIR — meaning whatever sits there is what the installed
    // binary will execute. Keep it in lockstep with our source so a
    // plain `cargo build` outside Nix is enough to refresh it before
    // the next `nixos-rebuild`. The file is gitignored; this is a
    // pure on-disk handoff.
    let manifest_lib = manifest_dir.join("libceleste_go.a");
    let manifest_hdr = manifest_dir.join("libceleste_go.h");
    std::fs::copy(&lib_path, &manifest_lib)
        .expect("failed to mirror libceleste_go.a into manifest dir");
    std::fs::copy(&header_path, &manifest_hdr)
        .expect("failed to mirror libceleste_go.h into manifest dir");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    // Rust strips the `lib` prefix and `.a` suffix before passing to the
    // linker, so `celeste_go` resolves to `libceleste_go.a`.
    println!("cargo:rustc-link-lib=static=celeste_go");

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
        .allowlist_function("ProtonDrive_Version")
        .allowlist_function("ProtonDrive_Login")
        .allowlist_function("ProtonDrive_Logout")
        .allowlist_function("ProtonDrive_SaveSession")
        .allowlist_function("ProtonDrive_SaveSessionIfRotated")
        .allowlist_function("ProtonDrive_ResumeSession")
        .allowlist_function("ProtonDrive_RootLinkID")
        .allowlist_function("ProtonDrive_ListDirectory")
        .allowlist_function("ProtonDrive_ListRecursive")
        .allowlist_function("ProtonDrive_Stat")
        .allowlist_function("ProtonDrive_DownloadFile")
        .allowlist_function("ProtonDrive_CreateFolder")
        .allowlist_function("ProtonDrive_UploadFile")
        .allowlist_function("ProtonDrive_TrashLink")
        .allowlist_function("ProtonDrive_PermanentDeleteLink")
        .allowlist_type("RcloneRPCResult")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Unable to generate bindings");
    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("failed to write bindings.rs");
}
