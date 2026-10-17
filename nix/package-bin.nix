# Syncative as released: the program from the release's AppImage, linked against nixpkgs' libraries instead of being compiled. Nix only downloads it, with no binary cache to set up. The source build is nix/package.nix (`#syncative-source`).
{
  lib,
  pkgs,
  stdenv,
  fetchurl,
  appimageTools,
  autoPatchelfHook,
  makeWrapper,
  makeDesktopItem,
  copyDesktopItems,
  dbus,
}:

let
  release = lib.importJSON ./release.json;
  inherit (release) version;

  appimage = appimageTools.extract {
    pname = "syncative";
    inherit version;
    src = fetchurl {
      url = "https://github.com/Santuzius/syncative/releases/download/v${version}/Syncative-${version}-x86_64.AppImage";
      inherit (release) hash;
    };
  };

  # The same run-time libraries as the source build (winit and wgpu load them while running).
  shell = import ../shell.nix { inherit pkgs; };

  desktopItem = makeDesktopItem {
    name = "syncative";
    desktopName = "Syncative";
    genericName = "File Synchronization Client";
    comment = "Sync local folders with Google Drive or Proton Drive";
    exec = "syncative --show";
    icon = "syncative-icon";
    categories = [ "Utility" "FileTransfer" "Network" ];
    keywords = [ "sync" "cloud" "backup" "rclone" ];
    startupWMClass = "syncative";
  };
in
stdenv.mkDerivation {
  pname = "syncative";
  inherit version;
  src = appimage;

  nativeBuildInputs = [ autoPatchelfHook makeWrapper copyDesktopItems ];
  # What the program links besides glibc: libdbus (keyring) and libgcc_s.
  buildInputs = [ dbus.lib stdenv.cc.cc.lib ];

  desktopItems = [ desktopItem ];

  installPhase = ''
    runHook preInstall
    install -Dm 755 usr/bin/syncative $out/bin/syncative
    install -Dm 644 syncative-icon.svg $out/share/icons/hicolor/scalable/apps/syncative-icon.svg
    runHook postInstall
  '';

  postFixup = ''
    wrapProgram $out/bin/syncative --prefix LD_LIBRARY_PATH : ${shell.LD_LIBRARY_PATH}
  '';

  meta = {
    description = "GUI file synchronization client for Google Drive and Proton Drive";
    homepage = "https://github.com/Santuzius/syncative";
    license = lib.licenses.gpl3Plus;
    mainProgram = "syncative";
    platforms = [ "x86_64-linux" ];
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
