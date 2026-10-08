# Celeste
Two-way file sync between folders on your Linux computer and Google Drive or Proton Drive, with a desktop window and a tray icon.

> [!NOTE]
> Personal fork of [hwittenborn/celeste](https://github.com/hwittenborn/celeste), maintained by Alexander Menzel; see [About this fork](#about-this-fork). This is a third-party application not officially supported by Google or Proton. Built and tested only on Linux.

![Celeste's main window in the dark theme, with two remotes and several synced folders](doc/img/Screenshot_dark.png)

## Quick start
1. **Install** with the [Nix package manager](https://nixos.org/download/) (works on any Linux distribution, flakes enabled):
   ```sh
   nix profile add github:Santuzius/celeste
   ```
   On NixOS, use the [module](#nixos) instead. To try Celeste without installing it, run `nix run github:Santuzius/celeste`.
2. **Start Celeste** from the app menu. It lives in the system tray; closing the window keeps it syncing in the background.
3. **Add a remote:** click *Add remote* and choose the provider.
   - **Proton Drive:** sign in with your Proton username and password.
   - **Google Drive:** needs your own OAuth client ID, which takes a few minutes in the Google Cloud console ([rclone's guide](https://rclone.org/drive/#making-your-own-client-id)). In the consent screen's **Branding** settings, enter [`https://github.com/Santuzius/celeste/blob/main/PRIVACY.md`](./PRIVACY.md) as **Application privacy policy link** and your own website or social media profile as **Application home page**, e.g. `https://t.me/YourTelegramName`. Then paste client ID and secret into Celeste, click *Connect* and grant access in your browser.
4. **Add a folder:** pick a folder on this computer and the path on the remote it should mirror. Missing folders are created on both sides, and Celeste keeps them in sync at the interval set in the remote's settings.

## Features
- Two-way sync, several folders per account, several accounts at the same time
- Exclusions for sub-folders and files, with a clean-up of copies left behind by an exclusion
- Conflict resolution: a file changed on both sides is never overwritten silently; you keep the local version, the remote one, or both
- Credentials are stored in the system keyring
- Runs in the tray, starts at login (can be switched off), Start / Pause per account
- Light and dark theme, following the system or chosen in Preferences

![The same window in the light theme](doc/img/Screenshot_light.png)

![Resolving a file that changed both locally and on Google Drive](doc/img/Screenshot_conflict.png)

## Installing
### Nix
`github:Santuzius/celeste` always gives the latest release; `nix profile upgrade` moves you to a newer one.

### NixOS
Add the flake as an input — this one line is all you need in `inputs`:

```nix
celeste.url = "github:Santuzius/celeste";
```

Then enable it through the NixOS module (installs Celeste, its menu entry and a system-wide autostart entry that starts it in the tray; Celeste's own per-user entry from Preferences takes precedence over it):

```nix
# flake.nix outputs, inside nixosSystem { modules = [ … ]; }
inputs.celeste.nixosModules.default
{ programs.celeste.enable = true; }   # programs.celeste.autostart = false; to skip autostart
```

Alternatively add `inputs.celeste.packages.${system}.default` to `environment.systemPackages` or use `overlays.default`.

### Snap
A Snap package is prepared (`snap/snapcraft.yaml`, see [doc/snap.md](doc/snap.md)) but not yet published in the Snap Store.

## Building
The project ships a `shell.nix` that provides a stable Rust toolchain, Go, and all runtime libraries (Wayland/Vulkan/X11, OpenSSL, rclone). From the repo root:

```sh
# Release build
nix-shell --run 'cargo build --release'

# Run directly
nix-shell --run 'cargo run --release'
```

No global `rustup`, `go`, or system headers are required — everything is pulled in by the shell.

## Used components
- [rclone](https://rclone.org/) for Google Drive (other rclone-supported providers would be possible, but are not implemented)
- [go-proton-api](https://github.com/ProtonMail/go-proton-api) for Proton Drive
- [iced](https://iced.rs/) for the GUI

## FAQ
### Why not also add other providers?
- As the sole maintainer I don't have enough time for this.
- Some of them already have a good Linux integration:
  - [OneDrive](https://abraunegg.github.io)
  - [Dropbox](https://www.dropbox.com/install-linux)
  - [Nextcloud](https://nextcloud.com/install/)
  - etc.

### Why not Flatpak or distribution packages?
This is my first published app, and packaging takes time; the Snap package is the next step.

## Privacy
Celeste runs entirely on your computer and sends nothing to the developer. See the [privacy policy](./PRIVACY.md).

## About this fork
This is a personal fork of the original project at [hwittenborn/celeste](https://github.com/hwittenborn/celeste), maintained by Alexander Menzel. See [`NOTICE`](./NOTICE) for the list of modifications relative to upstream. The fork is distributed under the same [GPL-3.0-or-later](./LICENSE) terms as the original.
