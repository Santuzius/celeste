# Privacy Policy

Last updated: 2026-10-01

Celeste is a desktop file synchronization client for Linux, maintained by Alexander Menzel ([github.com/Santuzius](https://github.com/Santuzius)). This policy describes how Celeste handles your data, including data it accesses through Google APIs.

## Summary

- Celeste runs entirely on your own computer. There is no Celeste server, no account with the developer, no telemetry, no analytics and no advertising.
- Celeste only talks to the cloud providers you connect (Google Drive, Proton Drive) to sync the folders you choose.
- The developer never receives, sees or stores any of your files, credentials or usage data.

## Data Celeste accesses

- **Files and folders** in the cloud storage you connect, and in the local folders you pair with them. Celeste reads, uploads, downloads, modifies and deletes files only to keep each folder pair in sync, as you configured it.
- **Account credentials:** OAuth tokens (Google Drive) and session tokens (Proton Drive). Your Proton password is used once to sign in and is not stored.
- **Google OAuth client ID and secret** that you create yourself and enter in Celeste.

## How data is stored

- Credentials and tokens are stored on your computer in the system keyring (Secret Service, e.g. GNOME Keyring or KWallet).
- Sync configuration and file metadata (paths, sizes, modification times, checksums) used to detect changes are stored in a local database in your user data directory.
- Nothing is stored anywhere else by Celeste.

## How data is shared

- File contents and metadata are transferred only between your computer and the cloud provider you connected, directly over encrypted connections (HTTPS).
- Celeste does not share, sell or transfer your data to the developer or any third party.
- Your use of each cloud provider is governed by that provider's own privacy policy ([Google](https://policies.google.com/privacy), [Proton](https://proton.me/legal/privacy)).

## Google API Services

Celeste's use and transfer of information received from Google APIs adheres to the [Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy), including the Limited Use requirements. Google user data is used solely to provide the sync feature you see in Celeste. It is not used for advertising, not used to train AI or machine-learning models, not read by humans, and not transferred to anyone.

## Deleting your data

- Removing a remote in Celeste deletes its stored credentials and its local sync state.
- You can revoke Celeste's access to your Google account at any time at [myaccount.google.com/permissions](https://myaccount.google.com/permissions).
- Uninstalling Celeste and deleting its data directory (`~/.local/share/celeste`) removes everything Celeste stored. Your synced files are left in place, locally and in the cloud.

## Children

Celeste is not directed at children under 13 and collects no personal information from anyone.

## Changes

Changes to this policy are published in this file; its history is available in the repository.

## Contact

Questions about this policy: open an issue at [github.com/Santuzius/celeste/issues](https://github.com/Santuzius/celeste/issues).
