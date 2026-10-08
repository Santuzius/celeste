# Privacy Policy

Last updated: 2026-10-08

Celeste is a desktop file synchronization client for Linux, maintained by Alexander Menzel ([github.com/Santuzius](https://github.com/Santuzius)). This policy describes how Celeste handles your data, including data it accesses through Google APIs.

## Summary

- Celeste runs entirely on your own computer. There is no Celeste server, no account with the developer, no telemetry, no analytics, no crash reporting, no update check and no advertising.
- Celeste only talks to the cloud providers you connect (Google Drive, Proton Drive) to sync the folders you choose.
- The developer never receives, sees or stores any of your files, credentials or usage data.

## Data Celeste accesses

- **Files and folders** in the cloud storage you connect, and in the local folders you pair with them. Celeste reads, uploads, downloads, modifies and deletes files only to keep each folder pair in sync, as you configured it, and to delete local leftovers of excluded folders when you ask it to.
- **Google Drive:** Celeste asks for access to your whole Drive (`https://www.googleapis.com/auth/drive`), because you can pair any folder of it, including files that Celeste did not create.
- **Account credentials:** OAuth tokens (Google Drive); session tokens and a key passphrase (Proton Drive, see below).
- **Google OAuth client ID and secret** that you create yourself and enter in Celeste.

## How data is stored

- Credentials are stored on your computer in the system keyring (Secret Service, e.g. GNOME Keyring or KWallet):
  - Google Drive: the OAuth access and refresh tokens and your client ID and secret.
  - Proton Drive: the session tokens and the salted key passphrase. Your Proton password is used once to sign in and is not stored, but the key passphrase derived from it is: it unlocks your Proton keys, so Celeste can decrypt and encrypt your files without asking for the password again. Anyone who can read your unlocked keyring can therefore read your Proton Drive files, just like your files on disk. Proton's own apps and rclone's Proton Drive backend keep the same kind of key.
- While Celeste runs, the Google Drive tokens are also written to `$XDG_RUNTIME_DIR/celeste/rclone.conf`, the configuration file of the rclone library Celeste uses. That directory is in memory, readable only by you, and emptied when you log out.
- A local database in your user data directory holds the sync configuration (connected accounts by name, folder pairs, exclusions, schedule) and, for every synced file, its local and cloud path and the modification times seen at the last sync, to detect changes.
- Celeste writes no log files. It prints diagnostic lines, which include file and folder names, to its standard error output. If your desktop starts Celeste (e.g. through autostart), the desktop may keep that output in the system journal or a session log.
- Preferences: the colour choices are kept in `appearance.conf` next to the database, and "Start Celeste when you log in" is the desktop's autostart entry `~/.config/autostart/celeste.desktop`.
- Nothing is stored anywhere else by Celeste.

## How data is shared

- File contents and metadata are transferred only between your computer and the cloud provider you connected, directly over encrypted connections (HTTPS).
- Proton Drive is end-to-end encrypted: Celeste encrypts file contents and names on your computer before upload, so Proton cannot read them.
- Like any client, Celeste reveals to the provider what the provider needs to serve it: your IP address, the files and folders it lists and transfers, and a client identification. Proton sees `external-drive-celeste` with the version, as Proton asks of third-party apps; Google sees the requests of the rclone library (below) under the OAuth client ID you entered.
- Celeste does not share, sell or transfer your data to the developer or any third party.
- Your use of each cloud provider is governed by that provider's own privacy policy ([Google](https://policies.google.com/privacy), [Proton](https://proton.me/legal/privacy)).

## Components from other projects

Celeste talks to Google Drive through [rclone](https://rclone.org), compiled into Celeste as a library, and to Proton Drive through Proton's [go-proton-api](https://github.com/ProtonMail/go-proton-api). Both run inside Celeste on your computer and only talk to the provider; neither sends anything to the rclone or Proton developers, and the rclone project runs no server that sees your data (see [rclone's privacy policy](https://rclone.org/privacy/)). What this means in practice:

- **Sign-in:** to connect Google Drive, Celeste runs `rclone authorize`, which opens Google's consent page in your browser and briefly listens on `127.0.0.1:53682` on your own computer to receive the result. The token comes directly from Google.
- **Client identification:** Google sees the rclone library's user agent (`rclone/v1.x`) together with your own client ID.
- **Older remotes:** Google Drive remotes added before Celeste required your own client ID still use rclone's shared client ID, so Google attributes their requests to the rclone project's OAuth client instead of yours; Celeste suggests switching them to your own. Dropbox, pCloud and WebDAV remotes created by older Celeste versions keep working through rclone and follow the same rules as above, with the respective provider in place of Google.

## Google API Services

Celeste's use and transfer of information received from Google APIs adheres to the [Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy), including the Limited Use requirements. Google user data is used solely to provide the sync feature you see in Celeste. It is not used for advertising, not used to train AI or machine-learning models, not read by humans, and not transferred to anyone.

## Deleting your data

- Removing a remote in Celeste deletes its stored credentials and its local sync state.
- You can revoke Celeste's access to your Google account at any time at [myaccount.google.com/permissions](https://myaccount.google.com/permissions). Signed-in Proton sessions can be ended in your Proton account settings under Security.
- Uninstalling Celeste and deleting its data directory (`~/.local/share/celeste`, or `~/snap/celeste` for the Snap) removes everything Celeste stored, apart from the autostart entry (`~/.config/autostart/celeste.desktop`). Your synced files are left in place, locally and in the cloud.

## Children

Celeste is not directed at children under 13 and collects no personal information from anyone.

## Changes

Changes to this policy are published in this file; its history is available in the repository.

## Contact

Questions about this policy: open an issue at [github.com/Santuzius/celeste/issues](https://github.com/Santuzius/celeste/issues).
