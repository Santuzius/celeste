//! Structs and functions for use with Rclone RPC calls.
//!
//! These calls are synchronous and block through librclone's FFI. Services
//! that call them are expected to run on a blocking tokio task
//! (`tokio::task::spawn_blocking`).
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use time::OffsetDateTime;

/// Get a remote from the config file.
pub fn get_remote<T: ToString>(remote: T) -> Option<Remote> {
    let remote = remote.to_string();

    let config_str = celeste_go::rpc(
        "config/get",
        json!({ "name": remote }).to_string(),
    )
    .unwrap();
    let config: HashMap<String, String> = serde_json::from_str(&config_str).unwrap();

    match config["type"].as_str() {
        "dropbox" => Some(Remote::Dropbox(DropboxRemote {
            remote_name: remote,
            client_id: config["client_id"].clone(),
            client_secret: config["client_secret"].clone(),
        })),
        "drive" => Some(Remote::GDrive(GDriveRemote {
            remote_name: remote,
            client_id: config["client_id"].clone(),
            client_secret: config["client_secret"].clone(),
        })),
        "pcloud" => Some(Remote::PCloud(PCloudRemote {
            remote_name: remote,
            client_id: config["client_id"].clone(),
            client_secret: config["client_secret"].clone(),
        })),
        "protondrive" => Some(Remote::ProtonDrive(ProtonDriveRemote {
            remote_name: remote,
            username: config["username"].clone(),
        })),
        "webdav" => {
            let vendor = match config["vendor"].as_str() {
                "nextcloud" => WebDavVendors::Nextcloud,
                "owncloud" => WebDavVendors::Owncloud,
                "webdav" => WebDavVendors::WebDav,
                _ => unreachable!(),
            };

            Some(Remote::WebDav(WebDavRemote {
                remote_name: remote,
                user: config["user"].clone(),
                pass: config["pass"].clone(),
                url: config["user"].clone(),
                vendor,
            }))
        }
        _ => None,
    }
}

/// Get all the remotes from the config file.
pub fn get_remotes() -> Vec<Remote> {
    let configs_str =
        celeste_go::rpc("config/listremotes", json!({}).to_string())
            .unwrap_or_else(|_| unreachable!());
    let configs = {
        let config: HashMap<String, Vec<String>> = serde_json::from_str(&configs_str).unwrap();
        config.get(&"remotes".to_string()).unwrap().to_owned()
    };
    let mut celeste_configs = vec![];

    for config in configs {
        celeste_configs.push(get_remote(&config).unwrap());
    }

    celeste_configs
}

/// The types of remotes in the config.
#[derive(Clone)]
pub enum Remote {
    Dropbox(DropboxRemote),
    GDrive(GDriveRemote),
    PCloud(PCloudRemote),
    ProtonDrive(ProtonDriveRemote),
    WebDav(WebDavRemote),
}

impl Remote {
    pub fn remote_name(&self) -> String {
        match self {
            Remote::Dropbox(remote) => remote.remote_name.clone(),
            Remote::GDrive(remote) => remote.remote_name.clone(),
            Remote::PCloud(remote) => remote.remote_name.clone(),
            Remote::ProtonDrive(remote) => remote.remote_name.clone(),
            Remote::WebDav(remote) => remote.remote_name.clone(),
        }
    }
}

// The Dropbox remote type.
#[derive(Clone, Debug)]
pub struct DropboxRemote {
    /// The name of the remote.
    pub remote_name: String,
    /// The client id.
    pub client_id: String,
    /// The client secret.
    pub client_secret: String,
}

// The Google Drive remote type.
#[derive(Clone, Debug)]
pub struct GDriveRemote {
    /// The name of the remote.
    pub remote_name: String,
    /// The client id.
    pub client_id: String,
    /// The client secret.
    pub client_secret: String,
}

// The pCloud remote type.
#[derive(Clone, Debug)]
pub struct PCloudRemote {
    /// The name of the remote.
    pub remote_name: String,
    /// The client id.
    pub client_id: String,
    /// The client secret.
    pub client_secret: String,
}

// The Proton Drive remote type.
#[derive(Clone, Debug)]
pub struct ProtonDriveRemote {
    /// The name of the remote.
    pub remote_name: String,
    /// the username.
    pub username: String,
}

// The WebDav remote type.
#[derive(Clone, Debug)]
pub struct WebDavRemote {
    /// The name of the remote.
    pub remote_name: String,
    /// The username for the remote.
    pub user: String,
    /// The password for the remote.
    pub pass: String,
    /// The URL for the remote.
    pub url: String,
    /// The vendor of the remote.
    pub vendor: WebDavVendors,
}

/// Possible WebDav vendors.
#[derive(Clone, Debug)]
pub enum WebDavVendors {
    Nextcloud,
    Owncloud,
    GDrive,
    PCloud,
    WebDav,
}

impl ToString for WebDavVendors {
    fn to_string(&self) -> String {
        match self {
            Self::Nextcloud => "Nextcloud",
            Self::Owncloud => "Owncloud",
            Self::GDrive => "Google Drive",
            Self::PCloud => "pCloud",
            Self::WebDav => "WebDav",
        }
        .to_string()
    }
}

/// Error returned from Rclone.
#[derive(Clone, Deserialize, Debug)]
pub struct RcloneError {
    pub error: String,
}

/// The output of an `operations/stat` command.
#[derive(Clone, Deserialize, Debug)]
pub struct BackendStat {
    item: Option<BackendRemoteItem>,
}

/// The output of an `operations/list` command.
#[derive(Clone, Deserialize, Debug)]
pub struct BackendList {
    #[serde(rename = "list")]
    list: Vec<BackendRemoteItem>,
}

/// The list of items in a folder, from the `list` object in the output of the
/// `operations/list` command.
#[derive(Clone, Deserialize, Debug)]
pub struct BackendRemoteItem {
    #[serde(rename = "IsDir")]
    pub is_dir: bool,
    #[serde(rename = "Path")]
    pub path: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "ModTime", with = "time::serde::rfc3339")]
    pub mod_time: OffsetDateTime,
    /// -1 when the backend doesn't know it.
    #[serde(rename = "Size", default)]
    pub size: i64,
    /// Only filled when requested (see [`sync::details`]); keys are rclone hash names such as `sha1`.
    #[serde(rename = "Hashes", default)]
    pub hashes: std::collections::HashMap<String, String>,
}

/// The types of items to show in an `operations/list` command.
#[derive(Clone, Debug)]
pub enum BackendListFilter {
    /// Return all items.
    All,
    /// Only return directories.
    Dirs,
    /// Only return files.
    #[allow(dead_code)]
    Files,
}

/// Functions for syncing to a remote. Each call is a blocking librclone
/// RPC — the caller is expected to be on a blocking tokio task.
pub mod sync {
    use super::{RcloneError, BackendList, BackendListFilter, BackendRemoteItem, BackendStat};
    use crate::util;
    use serde_json::json;

    /// Get a remote name.
    fn get_remote_name(remote: &str) -> String {
        if remote.ends_with(':') {
            panic!("Remote '{remote}' is not allowed to end with a ':'. Please omit it.",);
        }
        format!("{remote}:")
    }

    fn run<T: ToString>(method: T, input: T) -> Result<String, String> {
        celeste_go::rpc(method.to_string(), input.to_string())
    }

    /// Common function for some of the below command.
    fn common(command: &str, remote_name: &str, path: &str) -> Result<(), RcloneError> {
        let resp = run(
            command,
            &json!({
                "fs": get_remote_name(remote_name),
                "remote": util::strip_slashes(path),
            })
            .to_string(),
        );

        match resp {
            Ok(_) => Ok(()),
            Err(json_str) => Err(serde_json::from_str(&json_str).unwrap()),
        }
    }

    /// Delete a config.
    pub fn delete_config(remote_name: &str) -> Result<(), RcloneError> {
        let resp = run("config/delete", &json!({ "name": remote_name }).to_string());

        match resp {
            Ok(_) => Ok(()),
            Err(json_str) => Err(serde_json::from_str(&json_str).unwrap()),
        }
    }

    /// Get statistics about a file or folder.
    pub fn stat(remote_name: &str, path: &str) -> Result<Option<BackendRemoteItem>, RcloneError> {
        let resp = run(
            "operations/stat",
            &json!({
                "fs": get_remote_name(remote_name),
                "remote": util::strip_slashes(path)
            })
            .to_string(),
        );

        match resp {
            Ok(json_str) => Ok(serde_json::from_str::<BackendStat>(&json_str).unwrap().item),
            Err(json_str) => Err(serde_json::from_str(&json_str).unwrap()),
        }
    }

    /// Like [`stat`], with the SHA-1 of the content where the backend has one (Google Drive does).
    pub fn details(remote_name: &str, path: &str) -> Result<Option<BackendRemoteItem>, RcloneError> {
        let resp = run(
            "operations/stat",
            &json!({
                "fs": get_remote_name(remote_name),
                "remote": util::strip_slashes(path),
                "opt": { "showHash": true, "hashTypes": ["sha1"] }
            })
            .to_string(),
        );
        match resp {
            Ok(json_str) => Ok(serde_json::from_str::<BackendStat>(&json_str).unwrap().item),
            Err(json_str) => Err(serde_json::from_str(&json_str).unwrap()),
        }
    }

    /// List the files/folders in a path.
    pub fn list(
        remote_name: &str,
        path: &str,
        recursive: bool,
        filter: BackendListFilter,
    ) -> Result<Vec<BackendRemoteItem>, RcloneError> {
        let opts = match filter {
            BackendListFilter::All => json!({ "recurse": recursive }),
            BackendListFilter::Dirs => json!({"dirsOnly": true, "recurse": recursive}),
            BackendListFilter::Files => json!({"filesOnly": true, "recurse": recursive}),
        };

        let resp = run(
            "operations/list",
            &json!({
                "fs": get_remote_name(remote_name),
                "remote": util::strip_slashes(path),
                "opt": opts
            })
            .to_string(),
        );

        match resp {
            Ok(json_str) => Ok(serde_json::from_str::<BackendList>(&json_str).unwrap().list),
            Err(json_str) => Err(serde_json::from_str(&json_str).unwrap()),
        }
    }

    /// make a directory on the remote.
    pub fn mkdir(remote_name: &str, path: &str) -> Result<(), RcloneError> {
        common("operations/mkdir", remote_name, path)
    }

    /// Delete a single file.
    ///
    /// Must use `operations/deletefile`, NOT `operations/delete`:
    /// the latter is registered in rclone with `noRemote: true`
    /// (see fs/operations/rc.go), which means the `remote`
    /// parameter is silently ignored and `Delete(ctx, f)` lists
    /// and deletes every object in the entire `fs` root. Passing
    /// a per-file path to it wipes the whole remote.
    pub fn delete(remote_name: &str, path: &str) -> Result<(), RcloneError> {
        common("operations/deletefile", remote_name, path)
    }
    /// Remove a directory and all of its contents.
    pub fn purge(remote_name: &str, path: &str) -> Result<(), RcloneError> {
        common("operations/purge", remote_name, path)
    }

    /// Utility for copy functions.
    fn copy(
        src_fs: &str,
        src_remote: &str,
        dst_fs: &str,
        dst_remote: &str,
    ) -> Result<(), RcloneError> {
        let resp = run(
            "operations/copyfile",
            &json!({
                "srcFs": src_fs,
                "srcRemote": util::strip_slashes(src_remote),
                "dstFs": dst_fs,
                "dstRemote": util::strip_slashes(dst_remote)
            })
            .to_string(),
        );

        match resp {
            Ok(_) => Ok(()),
            Err(json_str) => Err(serde_json::from_str(&json_str).unwrap()),
        }
    }

    /// Copy a file from the local machine to the remote.
    pub fn copy_to_remote(
        local_file: &str,
        remote_name: &str,
        remote_destination: &str,
    ) -> Result<(), RcloneError> {
        copy(
            "/",
            local_file,
            &get_remote_name(remote_name),
            remote_destination,
        )
    }

    /// Copy a file from the remote to the local machine.
    pub fn copy_to_local(
        local_destination: &str,
        remote_name: &str,
        remote_file: &str,
    ) -> Result<(), RcloneError> {
        copy(
            &get_remote_name(remote_name),
            remote_file,
            "/",
            local_destination,
        )
    }
}
