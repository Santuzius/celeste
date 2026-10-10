# Celeste package, built fully inside the Nix sandbox: the Go c-archive (librclone + native Proton client) is built by src/go/build.rs from vendored Go modules, so no pre-built libceleste_go.a and no network access are needed.
{
  lib,
  pkgs,
  rustPlatform,
  buildGoModule,
  fetchFromGitHub,
  runCommand,
  makeWrapper,
  makeDesktopItem,
  copyDesktopItems,
  rclone,
  src,
}:

let
  cargoToml = lib.importTOML ../Cargo.toml;
  version = cargoToml.workspace.package.version;

  # The project's dev shell is the single source of build and runtime dependencies.
  shell = import ../shell.nix { inherit pkgs; };

  # src/go/proton-api is a git submodule, which flake sources (and GitHub tarballs) don't include; pin the same commit here. Keep `rev` in sync with `git submodule status`.
  protonApi = fetchFromGitHub {
    owner = "ProtonMail";
    repo = "go-proton-api";
    rev = "76509ffbc60cf6cf6d2bfba021b97ec8c05b8a5b";
    hash = "sha256-e6s3cMTfgkbOoxWMwkxj2W7s7j3rz65TMSCITzF3OoA=";
  };

  fullSrc = runCommand "celeste-source-${version}" { } ''
    cp -r ${src} $out
    chmod -R u+w $out
    rm -rf $out/src/go/proton-api $out/target $out/temp
    cp -r ${protonApi} $out/src/go/proton-api
  '';

  # Fixed-output derivation holding `go mod vendor` of src/go (including the replaced ./proton-api). Update `vendorHash` when src/go/go.mod or go.sum change: set it to lib.fakeHash, build, and copy the hash from the error.
  goModules =
    (buildGoModule {
      pname = "celeste-go";
      inherit version;
      src = fullSrc;
      modRoot = "src/go";
      vendorHash = "sha256-rpgGK2o/62M8UbKvP/NLsnTIICJRPV1RfGBYkjZUKvc=";
    }).goModules;

  desktopItem = makeDesktopItem {
    name = "celeste";
    desktopName = "Celeste";
    genericName = "File Synchronization Client";
    comment = "Sync local folders with Google Drive or Proton Drive";
    exec = "celeste --show";
    icon = "celeste-icon";
    categories = [ "Utility" "FileTransfer" "Network" ];
    keywords = [ "sync" "cloud" "backup" "rclone" ];
    startupWMClass = "celeste";
  };
in
rustPlatform.buildRustPackage {
  pname = "celeste";
  inherit version;
  src = fullSrc;

  cargoLock = {
    lockFile = ../Cargo.lock;
    # Git dependencies (the iced, iced_android and android-activity forks). After moving one to another commit, set its hash to lib.fakeHash, build, and copy the hash from the error.
    outputHashes = {
      "android-activity-0.6.1" = "sha256-3sn3y2ZDOt1GNH4mniv0vi18L089btGHt++/wLyQ3mw=";
      "iced-0.14.1" = "sha256-yBVh30RzNJd0dDuoKuQtuwok1TeCCGerH71jtku3jK0=";
      "iced_android-0.1.0" = "sha256-srhzPdXQcoifK5aOzvhn6HXB5WNxPjkxANQoSaC1Puk=";
    };

  # Some dependencies use unstable rustc features gated behind RUSTC_BOOTSTRAP.
  env.RUSTC_BOOTSTRAP = 1;

  # buildRustPackage brings its own Rust toolchain.
  nativeBuildInputs = builtins.filter (p: !(builtins.elem (p.pname or "") [ "rustc" "cargo" ])) shell.nativeBuildInputs ++ [
    makeWrapper
    copyDesktopItems
  ];
  buildInputs = shell.buildInputs;

  preBuild = ''
    cp -r --no-preserve=mode ${goModules} src/go/vendor
    export GOFLAGS=-mod=vendor GOPROXY=off GOTOOLCHAIN=local GOCACHE=$TMPDIR/go-cache
  '';

  desktopItems = [ desktopItem ];

  postInstall = ''
    install -Dm 644 assets/celeste-icon.svg $out/share/icons/hicolor/scalable/apps/celeste-icon.svg
  '';

  # winit/wgpu load Wayland, X11 and Vulkan libraries at run time; rclone must be on PATH for OAuth sign-in (`rclone authorize`).
  postFixup = ''
    wrapProgram $out/bin/celeste \
      --prefix PATH : ${lib.makeBinPath [ rclone ]} \
      --prefix LD_LIBRARY_PATH : ${shell.LD_LIBRARY_PATH}
  '';

  meta = {
    description = "GUI file synchronization client for Google Drive and Proton Drive";
    homepage = "https://github.com/Santuzius/celeste";
    license = lib.licenses.gpl3Plus;
    mainProgram = "celeste";
    platforms = lib.platforms.linux;
  };
}
