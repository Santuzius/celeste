{
  description = "Celeste — GUI file synchronization client for Google Drive and Proton Drive";

  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # x86_64 gets the released program (nix/package-bin.nix), so nothing is compiled; `celeste-source` builds it from this checkout. Other systems have no released program and always build.
      celesteFor = pkgs: if pkgs.stdenv.hostPlatform.system == "x86_64-linux" then pkgs.callPackage ./nix/package-bin.nix { } else sourceFor pkgs;
      sourceFor = pkgs: pkgs.callPackage ./nix/package.nix { src = self; };
    in
    {
      packages = forAllSystems (pkgs: rec {
        celeste = celesteFor pkgs;
        default = celeste;
        celeste-source = sourceFor pkgs;
      });

      overlays.default = final: _prev: {
        celeste = celesteFor final;
      };

      # NixOS: add `inputs.celeste.nixosModules.default` to your modules and set `programs.celeste.enable = true;`.
      nixosModules.default =
        { config, lib, pkgs, ... }:
        let
          cfg = config.programs.celeste;
        in
        {
          imports = [
            (lib.mkRemovedOptionModule [ "programs" "celeste" "binaryCache" ] "Celeste is now downloaded as released, with no binary cache to set up.")
          ];

          options.programs.celeste = {
            enable = lib.mkEnableOption "Celeste, a file synchronization client for Google Drive and Proton Drive";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.${if cfg.fromSource then "celeste-source" else "default"};
              defaultText = lib.literalExpression "inputs.celeste.packages.\${system}.default, or .celeste-source with fromSource";
              description = "The Celeste package to use.";
            };
            fromSource = lib.mkOption {
              type = lib.types.bool;
              default = false;
              description = "Build Celeste from source instead of downloading the released program (x86_64 only; other systems always build).";
            };
            autostart = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "Start Celeste in the system tray when a desktop session starts (XDG autostart).";
            };
          };

          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ cfg.package ];
            environment.etc."xdg/autostart/celeste.desktop" = lib.mkIf cfg.autostart {
              text = ''
                [Desktop Entry]
                Type=Application
                Name=Celeste
                Comment=Sync local folders with Google Drive or Proton Drive
                Exec=${lib.getExe cfg.package}
                Icon=celeste-icon
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
