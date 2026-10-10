{
  description = "Celeste — GUI file synchronization client for Google Drive and Proton Drive";

  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

  # Prebuilt packages for x86_64-linux. They match only with this flake's own nixpkgs, so leave out `inputs.nixpkgs.follows` to use them.
  nixConfig = {
    extra-substituters = [ "https://celeste.cachix.org" ];
    extra-trusted-public-keys = [ "celeste.cachix.org-1:iGmU8GUPr4AlGAiwmfAIDRhRKZwpIpWcz7DKwQBaRm8=" ];
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: rec {
        celeste = pkgs.callPackage ./nix/package.nix { src = self; };
        default = celeste;
      });

      overlays.default = final: _prev: {
        celeste = final.callPackage ./nix/package.nix { src = self; };
      };

      # NixOS: add `inputs.celeste.nixosModules.default` to your modules and set `programs.celeste.enable = true;`.
      nixosModules.default =
        { config, lib, pkgs, ... }:
        let
          cfg = config.programs.celeste;
        in
        {
          options.programs.celeste = {
            enable = lib.mkEnableOption "Celeste, a file synchronization client for Google Drive and Proton Drive";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
              defaultText = lib.literalExpression "inputs.celeste.packages.\${system}.default";
              description = "The Celeste package to use.";
            };
            autostart = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "Start Celeste in the system tray when a desktop session starts (XDG autostart).";
            };
            binaryCache = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "Fetch Celeste prebuilt from celeste.cachix.org instead of building it. Takes effect from the rebuild after the one that enables it, and only without `inputs.nixpkgs.follows` on the Celeste input.";
            };
          };

          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ cfg.package ];
            nix.settings = lib.mkIf cfg.binaryCache {
              substituters = [ "https://celeste.cachix.org" ];
              trusted-public-keys = [ "celeste.cachix.org-1:iGmU8GUPr4AlGAiwmfAIDRhRKZwpIpWcz7DKwQBaRm8=" ];
            };
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
