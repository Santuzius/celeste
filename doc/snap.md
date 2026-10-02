# Snap packaging

`snap/snapcraft.yaml` builds Celeste as a strictly confined snap on `core24` with the GNOME extension (Wayland, X11, GPU, portals and the GNOME runtime libraries come from shared content snaps, so the snap itself stays small).

## Building locally

Needs `snapcraft` and LXD (`sudo snap install snapcraft --classic && sudo snap install lxd && sudo lxd init --auto`), then from the repo root:

```sh
git submodule update --init src/go/proton-api
snapcraft pack
```

The result is `celeste_<version>_<arch>.snap`; the version comes from `[workspace.package]` in `Cargo.toml`.

## Trying it

```sh
sudo snap install --dangerous ./celeste_*.snap
sudo snap connect celeste:password-manager-service   # keyring access, not auto-connected
snap run celeste --show
```

What to check on a real desktop:

- Sign-in for Google Drive (browser opens, `rclone authorize` callback on 127.0.0.1:53682) and Proton Drive.
- The folder chooser returns real paths (e.g. `/home/you/Documents`) and not `/run/user/1000/doc/…`.
- The tray icon shows up (KDE, GNOME with the AppIndicator extension).
- Sync folders under the home directory work; hidden folders (`~/.something`) are not reachable with the `home` interface, `/media` and `/mnt` need `sudo snap connect celeste:removable-media`.
- `~` in the UI means the real home directory, not `~/snap/celeste/…`.

## Publishing

1. **Name.** The name `celeste` is registered to Hunter Wittenborn, the original author (last release 0.8.3 in June 2024). Either ask him to transfer it in the Snap Store dashboard, or register another name (e.g. `celeste-sync`) and change `name:` in `snapcraft.yaml`; with another name the command becomes `celeste-sync.celeste` unless an alias `celeste` is requested in the forum, and the data directory in `PRIVACY.md` changes to `~/snap/<name>`.
2. **Interfaces.** Ask on [forum.snapcraft.io](https://forum.snapcraft.io/c/store-requests/19) (category *store-requests*) for auto-connection of `password-manager-service` (sign-in tokens live in the Secret Service keyring; without it no remote can be connected). `home`, `network`, `network-bind`, `removable-media` (manual) and `unity7` need no request.
3. **Upload.** `snapcraft login`, then `snapcraft upload --release=stable celeste_<version>_amd64.snap` (and arm64, e.g. via `snapcraft remote-build`).
4. **Store page.** Screenshots (`doc/img/`), the privacy policy link (`PRIVACY.md`) and the contact are set in the dashboard.

Not covered yet: autostart. Snaps only start automatically when the app writes its desktop file to `~/snap/<name>/current/.config/autostart/` itself (with `autostart:` in `snapcraft.yaml`); Celeste has no autostart setting so far, the Nix module does it on NixOS.
