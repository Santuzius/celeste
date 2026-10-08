# Privacy Policy

Last updated: 2026-10-08

Celeste is a desktop file synchronization client for Linux, maintained by Alexander Menzel ([github.com/Santuzius](https://github.com/Santuzius)). This policy describes how Celeste handles your data, including data it accesses through Google APIs.

## Summary

- Celeste runs entirely on your own computer. There is no Celeste server, no account with the developer, no telemetry, no analytics, no crash reporting and no advertising.
- Celeste only talks to the cloud providers you connect (Google Drive, Proton Drive) to sync the folders you choose.
- The developer never receives, sees or stores any of your files, credentials or usage data.

## Data Celeste accesses

- **Files and folders** in the cloud storage you connect and in the local folders you pair with them, only to keep each pair in sync as you configured it.
- **Google Drive:** access to your whole Drive (`https://www.googleapis.com/auth/drive`), because you can pair any folder of it, including files Celeste did not create.
- **Credentials:** sign-in tokens for each account, the Google client ID and secret you enter, and for Proton Drive a key passphrase (below).

## How credentials are stored

- All credentials are stored in your system keyring (e.g. GNOME Keyring or KWallet), on your computer only.
- **Proton Drive:** your password is used once to sign in and is not stored. The key passphrase derived from it is, so Celeste can encrypt and decrypt your files without asking again. Anyone who can read your unlocked keyring can therefore read your Proton Drive files. Proton's own apps keep the same kind of key.
- **Google Drive:** while Celeste runs, its tokens are also kept in a temporary file that only you can read and that is deleted when you log out.
- Celeste also keeps a local database of your folder pairs and the names of synced files, to detect changes, and prints file names in its diagnostic output. Neither contains passwords, keys or tokens.

## How data is shared

- Files are transferred only between your computer and the provider you connected, over encrypted connections.
- Proton Drive is end-to-end encrypted: Celeste encrypts file contents and names before upload, so Proton cannot read them.
- Like any client, Celeste shows the provider your IP address, the files it transfers and an app identification.
- Google Drive is accessed through the open-source [rclone](https://rclone.org) library and Proton Drive through Proton's [go-proton-api](https://github.com/ProtonMail/go-proton-api). Both run inside Celeste and send nothing to their developers.
- Celeste does not share, sell or transfer your data to the developer or any third party.
- Your use of each provider is governed by its own privacy policy ([Google](https://policies.google.com/privacy), [Proton](https://proton.me/legal/privacy)).

## Google API Services

Celeste's use and transfer of information received from Google APIs adheres to the [Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy), including the Limited Use requirements. Google user data is used solely to provide the sync feature you see in Celeste. It is not used for advertising, not used to train AI or machine-learning models, not read by humans, and not transferred to anyone.

## Deleting your data

- Removing an account in Celeste deletes its credentials and its sync state.
- You can revoke Celeste's access to your Google account at [myaccount.google.com/permissions](https://myaccount.google.com/permissions). Proton sessions can be ended in your Proton account settings under Security.
- Uninstalling Celeste and deleting its data directory (`~/.local/share/celeste`, or `~/snap/celeste` for the Snap) removes everything else. Your synced files stay in place, locally and in the cloud.

## Children

Celeste is not directed at children under 13 and collects no personal information from anyone.

## Changes

Changes to this policy are published in this file; its history is available in the repository.

## Contact

Questions about this policy: open an issue at [github.com/Santuzius/celeste/issues](https://github.com/Santuzius/celeste/issues).
