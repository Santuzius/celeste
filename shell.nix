# Nix dev-shell for building Celeste on NixOS without a global toolchain
# install. Enter with `nix-shell` (or `nix-shell --run 'cargo build --release'`).
{ pkgs ? import <nixpkgs> { } }:

pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    rustc
    cargo
    pkg-config
    go
    rustPlatform.bindgenHook # sets LIBCLANG_PATH + BINDGEN_EXTRA_CLANG_ARGS for librclone-sys
  ];

  # Iced runtime libraries (winit/wgpu pick these at launch) plus librclone's
  # OpenSSL + the rclone CLI. `dbus` is needed by the `keyring` crate's
  # libsecret backend (libdbus-sys at build time, libdbus at runtime).
  buildInputs = with pkgs; [
    libxkbcommon
    vulkan-loader
    wayland
    libx11
    libxcursor
    libxi
    libxrandr
    fontconfig
    openssl
    rclone
    dbus
  ];

  # Some dependencies use unstable rustc features gated behind RUSTC_BOOTSTRAP.
  # Matches the value set in the celeste-nix package so dev builds mirror the
  # packaged build.
  RUSTC_BOOTSTRAP = 1;

  # Tell winit where to find the Wayland / Vulkan shared libs at run time.
  LD_LIBRARY_PATH = with pkgs;
    lib.makeLibraryPath [
      libxkbcommon
      vulkan-loader
      wayland
      libx11
      libxcursor
      libxi
      libxrandr
      fontconfig
    ];
}
