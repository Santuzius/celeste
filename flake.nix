{
  description = "Celeste — GUI file synchronization client for Google Drive and Proton Drive";

  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

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
