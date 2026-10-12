{
  description = "Syncative — GUI file synchronization client for Google Drive and Proton Drive";

  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # x86_64 gets the released program (nix/package-bin.nix), so nothing is compiled; `syncative-source` builds it from this checkout. Other systems have no released program and always build.
      syncativeFor = pkgs: if pkgs.stdenv.hostPlatform.system == "x86_64-linux" then pkgs.callPackage ./nix/package-bin.nix { } else sourceFor pkgs;
      sourceFor = pkgs: pkgs.callPackage ./nix/package.nix { src = self; };
    in
    {
      packages = forAllSystems (pkgs: rec {
        syncative = syncativeFor pkgs;
        default = syncative;
        syncative-source = sourceFor pkgs;
      });

      overlays.default = final: _prev: {
        syncative = syncativeFor final;
      };

      # NixOS: add `inputs.syncative.nixosModules.default` to your modules and set `programs.syncative.enable = true;`.
      nixosModules.default =
        { config, lib, pkgs, ... }:
        let
          cfg = config.programs.syncative;
        in
        {
          imports = [
            (lib.mkRemovedOptionModule [ "programs" "syncative" "binaryCache" ] "Syncative is now downloaded as released, with no binary cache to set up.")
          ];

          options.programs.syncative = {
            enable = lib.mkEnableOption "Syncative, a file synchronization client for Google Drive and Proton Drive";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.${if cfg.fromSource then "syncative-source" else "default"};
              defaultText = lib.literalExpression "inputs.syncative.packages.\${system}.default, or .syncative-source with fromSource";
              description = "The Syncative package to use.";
            };
            fromSource = lib.mkOption {
              type = lib.types.bool;
              default = false;
              description = "Build Syncative from source instead of downloading the released program (x86_64 only; other systems always build).";
            };
            autostart = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "Start Syncative in the system tray when a desktop session starts (XDG autostart).";
            };
          };

          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ cfg.package ];
            environment.etc."xdg/autostart/syncative.desktop" = lib.mkIf cfg.autostart {
              text = ''
                [Desktop Entry]
                Type=Application
                Name=Syncative
                Comment=Sync local folders with Google Drive or Proton Drive
                Exec=${lib.getExe cfg.package}
                Icon=syncative-icon
                Terminal=false
                X-GNOME-Autostart-enabled=true
              '';
            };
          };
        };

      devShells = forAllSystems (pkgs: {
        default = import ./shell.nix { inherit pkgs; };
      });
    };
}
